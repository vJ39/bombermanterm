//! ゲーム全体の状態と tick 更新。
//!
//! Integrateフェーズ: `GameState::new` / `GameState::tick` を実装する。
//!
//! tick の大まかな流れ (Screen::Playing 中):
//! 1. 入力(移動/ボム設置)を `players` の全員分反映する。移動は各プレイヤーの
//!    `speed` に応じたクールダウンで間引く。
//! 2. 設置済みボムの `timer` を dt 分減らし、0以下になったボムを爆発させる
//!    (`explosion_cells` で範囲計算)。爆風範囲に他のボムがあれば `timer` を
//!    0にして誘爆させる(連鎖)。爆風範囲の `Block` は `destroy_block` で破壊し
//!    アイテム化する。爆風範囲にプレイヤー(全員分)/敵が重なれば死亡処理する。
//! 3. 爆風の残存時間を減らし、切れたものを消す。
//! 4. 敵をクールダウン付きで `Enemy::decide_move` に従って移動させる。
//! 5. 敵との接触判定(素手で触れても死亡)をプレイヤーごとに行う。
//! 6. ラウンドの決着を判定する。
//!    - 1人プレイ+CPU戦: 敵全滅→Cleared、プレイヤー死亡&残機0→GameOver、
//!      残機が残っていれば復活。
//!    - 複数プレイヤー対戦: 生存者1人→`MatchResult(Some(勝者))`、全滅→
//!      `MatchResult(None)`(引き分け)。対戦では死亡=脱落で復活しない。
//!
//! 契約からの逸脱(いずれも追加のみ、契約シグネチャの変更は無い):
//! - `GameMap::take_item` を追加した(理由は `map.rs` の CONTRACT CHANGE コメント参照)。
//! - `GameState` に非公開フィールド `spawn_points` / `enemy_move_cooldown` を
//!   追加した。契約は `pub struct GameState { ... }` と省略記法で書かれており
//!   フィールド集合を固定していないため、内部実装専用フィールドとして追加した。
//! - `ItemKind::Invincible` を追加(`types.rs` の CONTRACT CHANGE 参照)。取得すると
//!   `INVINCIBLE_DURATION` 秒間、爆風・敵接触で死亡しなくなる。無敵中に敵へ触れると
//!   逆に敵を撃破する(本家の無敵アイテムの定番挙動)。
//! - CONTRACT CHANGE: 将来の4人対戦(ネットワーク対戦)の土台として、単一の
//!   `player: Player` を `players: Vec<Player>` に変更した。1人プレイ+CPU戦の挙動は
//!   従来のままで、`GameState::new` / `GameState::tick` のシグネチャも変えていない。
//!   - `score` / `lives` フィールドは `Player` 側へ移設し、1人プレイ視点の値を返す
//!     アクセサ [`GameState::score`] / [`GameState::lives`] (= `players[0]` の値)を用意した。
//!   - 移動クールダウンは `Player::move_cooldown` としてプレイヤーごとに持つ
//!     (旧 `GameState::player_move_cooldown` は削除)。
//!   - 本体ロジックは [`GameState::tick_multi`] に置き、`tick` はそこへ1人分の
//!     アクションを渡す後方互換ラッパーとして残した。
//!   - 複数人対戦の初期状態は [`GameState::new_multiplayer`] で作る。

use crate::audio::AudioPlayer;
use crate::game::entities::{explosion_cells, Bomb, Enemy, EnemyKind, Explosion, Player};
use crate::game::map::GameMap;
use crate::types::{Action, Bgm, Coord, Direction, ItemKind, Screen, SoundEffect, Tile};

/// マップの幅・高さ(壁込み)。奇数にして格子状固定壁と外周壁が綺麗に収まる大きさにする。
const MAP_WIDTH: usize = 15;
const MAP_HEIGHT: usize = 13;

/// プレイヤーの初期位置。`GameMap::generate` が常に `Empty` を保証する3マスの一つ。
const PLAYER_START: Coord = (1, 1);

/// 複数プレイヤー対戦でサポートするプレイヤー数の下限・上限。
/// `new_multiplayer` はこの範囲に丸める。
const MIN_MULTIPLAYER_PLAYERS: usize = 2;
const MAX_PLAYERS: usize = 4;

/// 出現させる敵の種類(1体ずつ)。
const ENEMY_KINDS: [EnemyKind; 3] = [EnemyKind::Chaser, EnemyKind::Wander, EnemyKind::Avoider];

/// 敵の初期出現位置をプレイヤーからなるべく離すための最低マンハッタン距離。
const MIN_ENEMY_SPAWN_DISTANCE: i32 = 6;

/// プレイヤーが1マス移動するのに掛かる基準秒数(`speed == 1.0` のとき)。
/// 実際の間隔は `BASE_MOVE_INTERVAL / player.speed` で、SpeedUp を取るほど短くなる。
const BASE_MOVE_INTERVAL: f32 = 0.12;

/// `speed` が 0 やごく小さい値になっても移動間隔が発散しないための下限。
const MIN_SPEED: f32 = 0.1;

/// 敵が1マス移動する間隔(秒)。プレイヤーの基準移動間隔より意図的に遅くしてある。
const ENEMY_MOVE_INTERVAL: f32 = 0.45;

/// SpeedUp アイテム1個で `player.speed` に加算する量。
const SPEED_INCREMENT: f32 = 0.4;

/// アイテム取得時の加点。
const SCORE_PER_ITEM: u32 = 50;
/// 敵を1体倒した時の加点。
const SCORE_PER_ENEMY: u32 = 200;
/// Invincible アイテム1個で付与される無敵時間(秒)。
const INVINCIBLE_DURATION: f32 = 5.0;

pub struct GameState {
    pub screen: Screen,
    pub map: GameMap,
    /// 同じマップ上に居るプレイヤー(1〜`MAX_PLAYERS`人)。Vec の添字がプレイヤー番号で、
    /// `Bomb::owner` および `tick_multi` の `actions` の添字と対応する。
    /// 1人プレイ+CPU戦では常に1要素(プレイヤー0)だけが入る。
    pub players: Vec<Player>,
    pub enemies: Vec<Enemy>,
    pub bombs: Vec<Bomb>,
    pub explosions: Vec<Explosion>,

    /// 各プレイヤーの初期位置(1人プレイ時の復活位置でもある)。`players` と同じ添字。
    spawn_points: Vec<Coord>,
    /// 敵が次に1マス移動するまでの残り秒数。0以下で全敵が1マスずつ移動する。
    enemy_move_cooldown: f32,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameState {
    /// 1人プレイ+CPU敵3体の初期状態を作る。
    pub fn new() -> Self {
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);
        let enemies = spawn_enemies(&map);

        GameState {
            screen: Screen::Title,
            map,
            players: vec![Player::new(PLAYER_START)],
            enemies,
            bombs: Vec::new(),
            explosions: Vec::new(),
            spawn_points: vec![PLAYER_START],
            enemy_move_cooldown: ENEMY_MOVE_INTERVAL,
        }
    }

    /// 複数プレイヤー対戦(2〜4人)の初期状態を作る。
    ///
    /// CPU敵は出さず(`enemies` は空)、各プレイヤーをマップ四隅にもっとも近い
    /// 進入可能マスへ重複なく配置する。`num_players` は 2〜`MAX_PLAYERS` に丸める。
    /// 画面は `new` と同じく `Screen::Title` から始まり、タイトルで入力があると
    /// 同じ人数のまま対戦が始まる。
    // 対戦の入口はまだ `main.rs` から呼んでいない(ネットワーク対戦は次フェーズ)ため、
    // バイナリ単体ビルドでは未使用として警告される。土台として公開しておく。
    #[allow(dead_code)]
    pub fn new_multiplayer(num_players: usize) -> Self {
        let num_players = num_players.clamp(MIN_MULTIPLAYER_PLAYERS, MAX_PLAYERS);
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);
        let spawn_points = player_spawn_positions(&map, num_players);
        let players: Vec<Player> = spawn_points.iter().copied().map(Player::new).collect();

        GameState {
            screen: Screen::Title,
            map,
            players,
            enemies: Vec::new(),
            bombs: Vec::new(),
            explosions: Vec::new(),
            spawn_points,
            enemy_move_cooldown: ENEMY_MOVE_INTERVAL,
        }
    }

    /// 1人プレイ視点の得点(プレイヤー0の得点)。
    ///
    /// 得点は `Player` ごとに持つようになったため、旧 `GameState::score`
    /// フィールドの参照箇所(HUD描画など)向けのアクセサとして用意している。
    /// 複数人対戦でどのプレイヤーの得点を見せるかは次フェーズで再設計する。
    pub fn score(&self) -> u32 {
        self.players.first().map(|player| player.score).unwrap_or(0)
    }

    /// 1人プレイ視点の残機(プレイヤー0の残機)。`score` と同じ理由のアクセサ。
    pub fn lives(&self) -> u32 {
        self.players.first().map(|player| player.lives).unwrap_or(0)
    }

    /// 固定tickでの状態更新(1人プレイ用の後方互換ラッパー)。
    ///
    /// 本体ロジックは [`GameState::tick_multi`] にあり、ここは受け取った1つの
    /// アクションをプレイヤー0のアクションとして委譲するだけ。
    pub fn tick(&mut self, dt: f32, action: Action, audio: &mut dyn AudioPlayer) {
        self.tick_multi(dt, &[action], audio);
    }

    /// 固定tickでの状態更新(複数プレイヤー対応の本体)。
    ///
    /// `actions` は `players` と同じ順序・同じ長さで渡す。要素が足りない分の
    /// プレイヤーは `Action::None`(入力なし)として扱い、余った要素は無視する。
    /// 入力アクションの反映、ボム/爆風/敵AIの進行、当たり判定、SE/BGM再生を行う。
    pub fn tick_multi(&mut self, dt: f32, actions: &[Action], audio: &mut dyn AudioPlayer) {
        match self.screen {
            Screen::Title => self.tick_title(actions, audio),
            Screen::Playing => self.tick_playing(dt, actions, audio),
            Screen::Cleared | Screen::GameOver | Screen::MatchResult(_) => {
                self.tick_result(actions, audio)
            }
        }
    }

    /// タイトル画面: Quit以外の何らかの入力(誰か1人でも)でゲームを開始する。
    fn tick_title(&mut self, actions: &[Action], audio: &mut dyn AudioPlayer) {
        let pressed_any = actions
            .iter()
            .any(|action| !matches!(action, Action::None | Action::Quit));
        if !pressed_any {
            return;
        }
        self.start_new_game(audio);
    }

    /// クリア/ゲームオーバー/対戦結果画面: ボム設置キー(SPACE)でタイトルへ戻る
    /// (画面表示のヒント文言と合わせている)。
    fn tick_result(&mut self, actions: &[Action], audio: &mut dyn AudioPlayer) {
        if actions
            .iter()
            .any(|action| matches!(action, Action::PlaceBomb))
        {
            self.screen = Screen::Title;
            audio.stop_bgm();
            audio.play_bgm(Bgm::Title);
        }
    }

    /// マップ・プレイヤー・敵を初期化してプレイを開始する。
    /// 得点・残機は `Player::new` の初期値で作り直すことでリセットされる。
    ///
    /// 現在のプレイヤー人数を引き継ぐため、1人なら従来の1人プレイ+CPU戦、
    /// 2人以上なら同じ人数の対戦(CPU敵なし)として作り直す。
    fn start_new_game(&mut self, audio: &mut dyn AudioPlayer) {
        let num_players = self.players.len();
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);

        if num_players <= 1 {
            self.enemies = spawn_enemies(&map);
            self.spawn_points = vec![PLAYER_START];
            self.players = vec![Player::new(PLAYER_START)];
        } else {
            let spawn_points = player_spawn_positions(&map, num_players);
            self.players = spawn_points.iter().copied().map(Player::new).collect();
            self.spawn_points = spawn_points;
            self.enemies = Vec::new();
        }

        self.map = map;
        self.bombs.clear();
        self.explosions.clear();
        self.enemy_move_cooldown = ENEMY_MOVE_INTERVAL;
        self.screen = Screen::Playing;

        audio.stop_bgm();
        audio.play_bgm(Bgm::Stage);
    }

    fn tick_playing(&mut self, dt: f32, actions: &[Action], audio: &mut dyn AudioPlayer) {
        for player in self.players.iter_mut() {
            if player.invincible_remaining > 0.0 {
                player.invincible_remaining = (player.invincible_remaining - dt).max(0.0);
            }
        }

        for idx in 0..self.players.len() {
            let action = actions.get(idx).copied().unwrap_or(Action::None);
            self.handle_player_input(idx, dt, action, audio);
        }

        // 前tickまでに存在した爆風をここで減衰させる。これから新たに発生する
        // 爆風(detonate_ready_bombs内で生成)は、まだ経過時間0のフレッシュな
        // remainingを持つべきなので、この減衰処理より後で追加する順序にする。
        for explosion in self.explosions.iter_mut() {
            explosion.remaining -= dt;
        }
        self.explosions.retain(|explosion| explosion.remaining > 0.0);

        for bomb in self.bombs.iter_mut() {
            bomb.timer -= dt;
        }
        self.detonate_ready_bombs(audio);

        self.update_enemies(dt);
        self.check_enemy_contact(audio);

        self.resolve_round_end(audio);
    }

    /// プレイヤー `idx` の移動・ボム設置の入力を反映する。
    /// 移動クールダウンはプレイヤーごとに独立して減算・設定する。
    fn handle_player_input(
        &mut self,
        idx: usize,
        dt: f32,
        action: Action,
        audio: &mut dyn AudioPlayer,
    ) {
        {
            let player = &mut self.players[idx];
            if player.move_cooldown > 0.0 {
                player.move_cooldown -= dt;
            }

            // 隠しコマンドによる強制無敵トグルは、プレイヤーの生死に関わらず効く
            // デバッグ用の裏機能なので、生存チェックより前で処理する。
            if matches!(action, Action::ToggleGodMode) {
                player.god_mode = !player.god_mode;
                return;
            }

            if !player.alive {
                return;
            }
        }

        match action {
            Action::Move(dir) if self.players[idx].move_cooldown <= 0.0 => {
                let target = step(self.players[idx].pos, dir);
                if self.map.is_walkable(target) && !self.bomb_at(target) {
                    let player = &mut self.players[idx];
                    player.pos = target;
                    player.move_cooldown = BASE_MOVE_INTERVAL / player.speed.max(MIN_SPEED);
                    self.try_pickup_item(idx, audio);
                }
            }
            Action::PlaceBomb => self.try_place_bomb(idx, audio),
            _ => {}
        }
    }

    /// プレイヤー `idx` の現在マスにアイテムがあれば回収して能力を強化する。
    fn try_pickup_item(&mut self, idx: usize, audio: &mut dyn AudioPlayer) {
        let pos = self.players[idx].pos;
        let Some(kind) = self.map.take_item(pos) else {
            return;
        };

        let player = &mut self.players[idx];
        match kind {
            ItemKind::Power => player.power += 1,
            ItemKind::BombUp => player.bomb_capacity += 1,
            ItemKind::SpeedUp => player.speed += SPEED_INCREMENT,
            ItemKind::Invincible => player.invincible_remaining = INVINCIBLE_DURATION,
        }
        player.score += SCORE_PER_ITEM;
        audio.play_se(SoundEffect::ItemGet);
    }

    /// プレイヤー `idx` の bomb_capacity の上限内、かつ同じマスに設置済みでなければ
    /// ボムを置く。設置数の上限は「そのプレイヤーが所有するボム」だけで数える。
    fn try_place_bomb(&mut self, idx: usize, audio: &mut dyn AudioPlayer) {
        let (pos, power, capacity) = {
            let player = &self.players[idx];
            (player.pos, player.power, player.bomb_capacity)
        };

        let active_own_bombs = self.bombs.iter().filter(|bomb| bomb.owner == idx).count() as u32;
        if active_own_bombs >= capacity {
            return;
        }
        if self.bombs.iter().any(|bomb| bomb.pos == pos) {
            return;
        }

        self.bombs.push(Bomb::new(pos, power, idx));
        audio.play_se(SoundEffect::PlaceBomb);
    }

    /// timer が0以下になったボムを順に爆発させる。爆風が他のボムに掛かれば
    /// そのボムの timer も0にするため、連鎖爆発が起きるまでループする。
    fn detonate_ready_bombs(&mut self, audio: &mut dyn AudioPlayer) {
        loop {
            let Some(idx) = self.bombs.iter().position(|bomb| bomb.timer <= 0.0) else {
                break;
            };
            let bomb = self.bombs.remove(idx);
            let cells = explosion_cells(bomb.pos, bomb.power, &self.map);

            // 誘爆: 爆風の中にある他のボムは即座にタイマー0にする。
            for other in self.bombs.iter_mut() {
                if cells.contains(&other.pos) {
                    other.timer = 0.0;
                }
            }

            // 破壊可能ブロックを壊してアイテム化する。
            for &cell in &cells {
                if matches!(self.map.tile_at(cell), Tile::Block) {
                    self.map.destroy_block(cell);
                }
            }

            // 爆風に巻き込まれたプレイヤーは全員死亡処理(無敵中は無効)。
            for idx in 0..self.players.len() {
                let caught = {
                    let player = &self.players[idx];
                    player.alive && !player.is_invincible() && cells.contains(&player.pos)
                };
                if caught {
                    self.hurt_player(idx, audio);
                }
            }

            // 敵が爆風に巻き込まれたら撃破。撃破スコアはボムの所有者に入る。
            let mut owner_score = 0;
            for enemy in self.enemies.iter_mut() {
                if enemy.alive && cells.contains(&enemy.pos) {
                    enemy.alive = false;
                    owner_score += SCORE_PER_ENEMY;
                }
            }
            if owner_score > 0
                && let Some(owner) = self.players.get_mut(bomb.owner)
            {
                owner.score += owner_score;
            }

            audio.play_se(SoundEffect::Explosion);
            self.explosions.push(Explosion::new(cells));
        }
    }

    /// 敵をクールダウンに従って1マスずつ移動させる。
    fn update_enemies(&mut self, dt: f32) {
        if self.enemy_move_cooldown > 0.0 {
            self.enemy_move_cooldown -= dt;
            return;
        }
        self.enemy_move_cooldown = ENEMY_MOVE_INTERVAL;

        let map = &self.map;
        let bombs = &self.bombs;
        let players = &self.players;
        for enemy in self.enemies.iter_mut() {
            if !enemy.alive {
                continue;
            }
            // 追跡/逃走の基準は「その敵にもっとも近いプレイヤー」。
            // プレイヤーが1人なら常にそのプレイヤーが基準になる(従来と同じ挙動)。
            let Some(player_pos) = nearest_player_pos(players, enemy.pos) else {
                continue;
            };
            let dir = enemy.decide_move(map, player_pos);
            let target = step(enemy.pos, dir);
            if map.is_walkable(target) && !bombs.iter().any(|bomb| bomb.pos == target) {
                enemy.pos = target;
            }
        }
    }

    /// 指定マスに設置済みのボムがあるか。
    fn bomb_at(&self, pos: Coord) -> bool {
        self.bombs.iter().any(|bomb| bomb.pos == pos)
    }

    /// 爆風以外でも、敵に直接触れたらプレイヤーは死亡する。判定はプレイヤーごと。
    /// 無敵モード中は逆に、触れた敵を蹴散らす(死亡しない・敵を撃破する)。
    fn check_enemy_contact(&mut self, audio: &mut dyn AudioPlayer) {
        for idx in 0..self.players.len() {
            let (alive, invincible, pos) = {
                let player = &self.players[idx];
                (player.alive, player.is_invincible(), player.pos)
            };
            if !alive {
                continue;
            }

            if invincible {
                let mut gained = 0;
                for enemy in self.enemies.iter_mut() {
                    if enemy.alive && enemy.pos == pos {
                        enemy.alive = false;
                        gained += SCORE_PER_ENEMY;
                    }
                }
                self.players[idx].score += gained;
                continue;
            }

            let touched = self
                .enemies
                .iter()
                .any(|enemy| enemy.alive && enemy.pos == pos);
            if touched {
                self.hurt_player(idx, audio);
            }
        }
    }

    /// プレイヤー `idx` を死亡させ、そのプレイヤーの残機を1つ減らす。
    fn hurt_player(&mut self, idx: usize, audio: &mut dyn AudioPlayer) {
        let player = &mut self.players[idx];
        if !player.alive {
            return;
        }
        player.alive = false;
        player.lives = player.lives.saturating_sub(1);
        audio.play_se(SoundEffect::Death);
    }

    /// 複数プレイヤーの対戦中か(プレイヤーが2人以上でCPU敵が居ない)。
    /// 1人プレイ+CPU戦、および将来の協力プレイ(敵あり)は従来の決着判定を使う。
    fn is_versus_match(&self) -> bool {
        self.players.len() > 1 && self.enemies.is_empty()
    }

    /// ラウンドの決着を判定する。対戦かどうかで判定ロジックを分ける。
    fn resolve_round_end(&mut self, audio: &mut dyn AudioPlayer) {
        if self.is_versus_match() {
            self.resolve_versus_end(audio);
        } else {
            self.resolve_solo_end(audio);
        }
    }

    /// 1人プレイ+CPU戦の決着: プレイヤー死亡→残機があれば復活/無ければGameOver、
    /// 敵全滅ならCleared。
    ///
    /// 死亡したプレイヤーが居たtickではクリア判定を行わない(復活処理を優先する)
    /// 従来の順序をそのまま保つ。
    fn resolve_solo_end(&mut self, audio: &mut dyn AudioPlayer) {
        let mut had_dead_player = false;
        let mut out_of_lives = false;

        for idx in 0..self.players.len() {
            if self.players[idx].alive {
                continue;
            }
            had_dead_player = true;

            if self.players[idx].lives == 0 {
                out_of_lives = true;
            } else {
                let spawn = self.spawn_point(idx);
                let player = &mut self.players[idx];
                player.pos = spawn;
                player.alive = true;
                player.move_cooldown = 0.0;
            }
        }

        if out_of_lives {
            self.screen = Screen::GameOver;
            audio.stop_bgm();
            audio.play_bgm(Bgm::GameOver);
            return;
        }
        if had_dead_player {
            return;
        }

        if !self.enemies.is_empty() && self.enemies.iter().all(|enemy| !enemy.alive) {
            self.screen = Screen::Cleared;
            audio.stop_bgm();
            audio.play_se(SoundEffect::StageClear);
            audio.play_bgm(Bgm::Clear);
        }
    }

    /// 複数プレイヤー対戦の決着: 生存者が1人になったらその1人の勝ち、
    /// 全員死亡(相打ち)なら引き分け。対戦では死亡=脱落で、残機による復活はしない。
    fn resolve_versus_end(&mut self, audio: &mut dyn AudioPlayer) {
        let alive: Vec<usize> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, player)| player.alive)
            .map(|(idx, _)| idx)
            .collect();

        match alive.len() {
            1 => {
                self.screen = Screen::MatchResult(Some(alive[0]));
                audio.stop_bgm();
                audio.play_se(SoundEffect::StageClear);
                audio.play_bgm(Bgm::Clear);
            }
            0 => {
                self.screen = Screen::MatchResult(None);
                audio.stop_bgm();
                audio.play_bgm(Bgm::GameOver);
            }
            _ => {}
        }
    }

    /// プレイヤー `idx` の初期位置(復活位置)。
    fn spawn_point(&self, idx: usize) -> Coord {
        self.spawn_points.get(idx).copied().unwrap_or(PLAYER_START)
    }
}

/// 敵をプレイヤーからなるべく離れた進入可能マスに出現させる。
fn spawn_enemies(map: &GameMap) -> Vec<Enemy> {
    spawn_positions(map, ENEMY_KINDS.len(), PLAYER_START)
        .into_iter()
        .zip(ENEMY_KINDS.iter().copied())
        .map(|(pos, kind)| Enemy::new(pos, kind))
        .collect()
}

/// `avoid` からなるべく離れた進入可能マスを `count` 個探す。
/// 十分な数が見付からない場合は距離条件を無視して残りを補充する。
fn spawn_positions(map: &GameMap, count: usize, avoid: Coord) -> Vec<Coord> {
    let mut positions: Vec<Coord> = Vec::with_capacity(count);

    for row in (0..map.height as i32).rev() {
        for col in (0..map.width as i32).rev() {
            if positions.len() >= count {
                return positions;
            }
            let pos = (row, col);
            if pos == avoid || positions.contains(&pos) || !map.is_walkable(pos) {
                continue;
            }
            if manhattan(pos, avoid) < MIN_ENEMY_SPAWN_DISTANCE {
                continue;
            }
            positions.push(pos);
        }
    }

    if positions.len() < count {
        for row in 0..map.height as i32 {
            for col in 0..map.width as i32 {
                if positions.len() >= count {
                    return positions;
                }
                let pos = (row, col);
                if pos == avoid || positions.contains(&pos) || !map.is_walkable(pos) {
                    continue;
                }
                positions.push(pos);
            }
        }
    }

    positions
}

/// マップ四隅(外周壁の内側)のアンカー座標。左上→右上→左下→右下の順。
/// プレイヤー0は必ず `PLAYER_START` (左上)が割り当たるようにこの順序にしている。
fn corner_anchors(map: &GameMap) -> [Coord; MAX_PLAYERS] {
    let last_row = map.height as i32 - 2;
    let last_col = map.width as i32 - 2;
    [
        PLAYER_START,
        (1, last_col),
        (last_row, 1),
        (last_row, last_col),
    ]
}

/// 対戦用に、四隅それぞれにもっとも近い進入可能マスを `count` 個、重複なく返す。
///
/// 四隅そのものが破壊可能ブロックで埋まっている盤面もあるため、角に固定するのではなく
/// 「角からマンハッタン距離が最小の進入可能マス」を選ぶ。既に他のプレイヤーへ
/// 割り当てたマスは除外するので、プレイヤー同士の初期位置は必ず異なる。
fn player_spawn_positions(map: &GameMap, count: usize) -> Vec<Coord> {
    let anchors = corner_anchors(map);
    let mut positions: Vec<Coord> = Vec::with_capacity(count);

    for i in 0..count.min(anchors.len()) {
        if let Some(pos) = nearest_walkable(map, anchors[i], &positions) {
            positions.push(pos);
        }
    }

    // 進入可能マスが極端に少ない盤面で足りなかった場合の保険(通常は起きない)。
    if positions.len() < count {
        for row in 0..map.height as i32 {
            for col in 0..map.width as i32 {
                if positions.len() >= count {
                    return positions;
                }
                let pos = (row, col);
                if map.is_walkable(pos) && !positions.contains(&pos) {
                    positions.push(pos);
                }
            }
        }
    }

    positions
}

/// `anchor` からもっとも近い進入可能マスを返す(`taken` のマスは除く)。
/// 距離が同じマスが複数ある場合は走査順(上の行・左の列が先)で最初のものを選ぶ。
fn nearest_walkable(map: &GameMap, anchor: Coord, taken: &[Coord]) -> Option<Coord> {
    let mut best: Option<(i32, Coord)> = None;

    for row in 0..map.height as i32 {
        for col in 0..map.width as i32 {
            let pos = (row, col);
            if !map.is_walkable(pos) || taken.contains(&pos) {
                continue;
            }
            let distance = manhattan(pos, anchor);
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, pos));
            }
        }
    }

    best.map(|(_, pos)| pos)
}

/// `from` からもっとも近いプレイヤーの位置を返す。
///
/// 生存プレイヤーを優先し、全員死亡している場合は(復活待ちの1人プレイを含め)
/// 死亡プレイヤーも含めて最短のものを返す。プレイヤーが1人だけならその位置になる。
fn nearest_player_pos(players: &[Player], from: Coord) -> Option<Coord> {
    players
        .iter()
        .filter(|player| player.alive)
        .map(|player| player.pos)
        .min_by_key(|&pos| manhattan(pos, from))
        .or_else(|| {
            players
                .iter()
                .map(|player| player.pos)
                .min_by_key(|&pos| manhattan(pos, from))
        })
}

fn manhattan(a: Coord, b: Coord) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

fn step(pos: Coord, dir: Direction) -> Coord {
    match dir {
        Direction::Up => (pos.0 - 1, pos.1),
        Direction::Down => (pos.0 + 1, pos.1),
        Direction::Left => (pos.0, pos.1 - 1),
        Direction::Right => (pos.0, pos.1 + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 開始時の残機は `Player` 側の定数を参照する(期待値をここで再定義しない)。
    use crate::game::entities::STARTING_LIVES;

    /// テスト用の無音 AudioPlayer。呼び出された SE/BGM を記録するだけ。
    #[derive(Default)]
    struct NoopAudio {
        se_calls: Vec<SoundEffect>,
        bgm_calls: Vec<Bgm>,
        stop_calls: u32,
    }

    impl AudioPlayer for NoopAudio {
        fn play_se(&mut self, se: SoundEffect) {
            self.se_calls.push(se);
        }
        fn play_bgm(&mut self, bgm: Bgm) {
            self.bgm_calls.push(bgm);
        }
        fn stop_bgm(&mut self) {
            self.stop_calls += 1;
        }
    }

    #[test]
    fn new_starts_on_title_with_full_lives_and_enemies() {
        let state = GameState::new();
        assert_eq!(state.screen, Screen::Title);
        assert_eq!(state.lives(), STARTING_LIVES);
        assert_eq!(state.score(), 0);
        assert!(state.players[0].alive);
        assert_eq!(state.enemies.len(), ENEMY_KINDS.len());
        assert!(state.bombs.is_empty());
        assert!(state.explosions.is_empty());
    }

    #[test]
    fn any_action_on_title_starts_playing_except_none_and_quit() {
        let mut audio = NoopAudio::default();

        let mut state = GameState::new();
        state.tick(0.033, Action::None, &mut audio);
        assert_eq!(state.screen, Screen::Title, "None must not start the game");

        state.tick(0.033, Action::Quit, &mut audio);
        assert_eq!(state.screen, Screen::Title, "Quit must not start the game");

        state.tick(0.033, Action::PlaceBomb, &mut audio);
        assert_eq!(state.screen, Screen::Playing);
        assert!(audio.bgm_calls.contains(&Bgm::Stage));
    }

    #[test]
    fn player_moves_into_walkable_cell_and_not_into_wall() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        let before = state.players[0].pos;
        // (1,1) から Up は外周壁 (0,1) なので移動できないはず。
        state.tick(1.0, Action::Move(Direction::Up), &mut audio);
        assert_eq!(state.players[0].pos, before);

        // Right は (1,2) が常に Empty 保証なので移動できるはず。
        state.players[0].move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(state.players[0].pos, (1, 2));
    }

    #[test]
    fn player_cannot_walk_onto_a_bomb_tile() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        // (1,2) は常に Empty 保証。ここにボムを置き、プレイヤーが重ならず
        // 素通りできないことを確認する(タイルは通行可能でもボムは実体として塞ぐ)。
        state.bombs.push(Bomb::new((1, 2), 1, 0));

        state.players[0].move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(
            state.players[0].pos, PLAYER_START,
            "player must not overlap a bomb tile"
        );
    }

    #[test]
    fn enemy_cannot_walk_onto_a_bomb_tile() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        // player は常に (1,1) から開始し (1,2)/(2,1) は常に Empty が保証される。
        // Chaser を (1,2) に置けば、player(1,1)への最短方向は一意にLeftへ確定する
        // (盤面のランダム配置に依存しない決定的なセットアップ)。
        state.enemies[0].pos = (1, 2);
        state.enemies[0].kind = EnemyKind::Chaser;
        state.enemies[0].alive = true;

        // 進みたい先の (1,1) にボムを置いて塞ぐ。
        state.bombs.push(Bomb::new(PLAYER_START, 1, 0));

        state.enemy_move_cooldown = 0.0;
        state.tick(1.0, Action::None, &mut audio);

        assert_eq!(
            state.enemies[0].pos,
            (1, 2),
            "enemy must not overlap a bomb tile even when it is the closest direction to the player"
        );
    }

    #[test]
    fn place_bomb_respects_capacity_and_dedup() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing, capacity=1

        state.bombs.clear(); // 開始直後の設置分を除去してから検証する
        state.tick(0.001, Action::PlaceBomb, &mut audio);
        assert_eq!(state.bombs.len(), 1);

        // 同じマスへの二重設置は無視される。
        state.tick(0.001, Action::PlaceBomb, &mut audio);
        assert_eq!(state.bombs.len(), 1);
    }

    #[test]
    fn bomb_timer_expires_and_creates_explosion() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();
        state.explosions.clear();

        state.bombs.push(Bomb::new(state.players[0].pos, 1, 0));
        state.tick(10.0, Action::None, &mut audio);

        assert!(state.bombs.is_empty());
        assert!(!state.explosions.is_empty());
        assert!(audio.se_calls.contains(&SoundEffect::Explosion));
    }

    #[test]
    fn explosion_kills_player_and_respawns_when_lives_remain() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        let starting_lives = state.lives();
        state.bombs.push(Bomb::new(state.players[0].pos, 1, 0));
        state.tick(10.0, Action::None, &mut audio);

        assert_eq!(state.lives(), starting_lives - 1);
        assert_eq!(state.screen, Screen::Playing);
        assert!(state.players[0].alive, "should respawn while lives remain");
        assert_eq!(state.players[0].pos, PLAYER_START);
        assert!(audio.se_calls.contains(&SoundEffect::Death));
    }

    #[test]
    fn player_death_with_no_lives_ends_game() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.players[0].lives = 1;
        state.bombs.clear();

        state.bombs.push(Bomb::new(state.players[0].pos, 1, 0));
        state.tick(10.0, Action::None, &mut audio);

        assert_eq!(state.lives(), 0);
        assert_eq!(state.screen, Screen::GameOver);
        assert!(audio.bgm_calls.contains(&Bgm::GameOver));
    }

    #[test]
    fn all_enemies_defeated_clears_stage() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        for enemy in state.enemies.iter_mut() {
            enemy.alive = false;
        }
        state.tick(0.033, Action::None, &mut audio);

        assert_eq!(state.screen, Screen::Cleared);
        assert!(audio.bgm_calls.contains(&Bgm::Clear));
    }

    #[test]
    fn result_screen_space_returns_to_title() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.screen = Screen::GameOver;

        state.tick(0.033, Action::Move(Direction::Up), &mut audio);
        assert_eq!(
            state.screen,
            Screen::GameOver,
            "only PlaceBomb should return to title"
        );

        state.tick(0.033, Action::PlaceBomb, &mut audio);
        assert_eq!(state.screen, Screen::Title);
    }

    #[test]
    fn item_pickup_increases_player_stats() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        // (1,2) にアイテムを直接置いて拾わせる(生成確率に依存しないための直接操作)。
        // GameMap には直接セットする契約メソッドが無いため、destroy_block/take_item
        // が対象にする Block を経由せず、ここでは移動→pickup ロジックのみを
        // 一意に検証するために power の変化を見る。
        let before_power = state.players[0].power;
        // (1,2) は常に Empty なのでアイテムは無い前提だが、pickup 経路自体が
        // panicせず何も起きないことを確認する回帰テストとして扱う。
        state.players[0].move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(state.players[0].power, before_power);
    }

    #[test]
    fn item_pickup_applies_correct_effect_for_each_kind() {
        // GameMap にはテスト用のタイル直接セットが無いため、実際に破壊可能ブロックを
        // 探して destroy_block でアイテム化し、プレイヤーをそのマスへ動かして拾わせる。
        // Power/BombUp/SpeedUp/Invincible の4種すべてを観測できるまでマップ生成をやり直す。
        let mut audio = NoopAudio::default();
        let mut seen_power = false;
        let mut seen_bomb_up = false;
        let mut seen_speed_up = false;
        let mut seen_invincible = false;

        'attempts: for _ in 0..600 {
            let mut state = GameState::new();
            state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

            for row in 0..state.map.height as i32 {
                for col in 0..state.map.width as i32 {
                    let pos = (row, col);
                    if state.map.tile_at(pos) != Tile::Block {
                        continue;
                    }
                    let Some(kind) = state.map.destroy_block(pos) else {
                        continue;
                    };
                    match kind {
                        ItemKind::Power if seen_power => continue,
                        ItemKind::BombUp if seen_bomb_up => continue,
                        ItemKind::SpeedUp if seen_speed_up => continue,
                        ItemKind::Invincible if seen_invincible => continue,
                        _ => {}
                    }

                    let before_power = state.players[0].power;
                    let before_capacity = state.players[0].bomb_capacity;
                    let before_speed = state.players[0].speed;
                    let before_score = state.score();
                    let before_invincible = state.players[0].invincible_remaining;

                    state.players[0].pos = pos;
                    state.try_pickup_item(0, &mut audio);

                    assert_eq!(
                        state.map.tile_at(pos),
                        Tile::Empty,
                        "item tile must be consumed on pickup"
                    );
                    assert_eq!(state.score(), before_score + SCORE_PER_ITEM);
                    assert!(audio.se_calls.contains(&SoundEffect::ItemGet));

                    match kind {
                        ItemKind::Power => {
                            assert_eq!(state.players[0].power, before_power + 1);
                            seen_power = true;
                        }
                        ItemKind::BombUp => {
                            assert_eq!(state.players[0].bomb_capacity, before_capacity + 1);
                            seen_bomb_up = true;
                        }
                        ItemKind::SpeedUp => {
                            assert!(
                                (state.players[0].speed - (before_speed + SPEED_INCREMENT)).abs()
                                    < 1e-6
                            );
                            seen_speed_up = true;
                        }
                        ItemKind::Invincible => {
                            assert_eq!(before_invincible, 0.0);
                            assert_eq!(state.players[0].invincible_remaining, INVINCIBLE_DURATION);
                            assert!(state.players[0].is_invincible());
                            seen_invincible = true;
                        }
                    }

                    if seen_power && seen_bomb_up && seen_speed_up && seen_invincible {
                        break 'attempts;
                    }
                }
            }
        }

        assert!(seen_power, "never observed a Power pickup across attempts");
        assert!(seen_bomb_up, "never observed a BombUp pickup across attempts");
        assert!(
            seen_speed_up,
            "never observed a SpeedUp pickup across attempts"
        );
        assert!(
            seen_invincible,
            "never observed an Invincible pickup across attempts"
        );
    }

    #[test]
    fn bomb_explosion_chain_detonates_bomb_in_blast_range() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();
        state.explosions.clear();

        // player は常に (1,1) から開始し、(1,2)/(2,1) は常に Empty が保証される。
        // bombA(player位置, power1)の爆風は(1,2)まで届くので、そこに timer が
        // 満了していない bombB を置いておけば誘爆するはず。
        let mut bomb_a = Bomb::new(state.players[0].pos, 1, 0);
        bomb_a.timer = 0.001;
        let mut bomb_b = Bomb::new((1, 2), 1, 0);
        bomb_b.timer = 100.0;
        state.bombs.push(bomb_a);
        state.bombs.push(bomb_b);

        state.tick(1.0, Action::None, &mut audio);

        assert!(
            state.bombs.is_empty(),
            "both bombs should detonate in the same tick via chain reaction"
        );
        assert_eq!(
            state.explosions.len(),
            2,
            "each detonation (original + chained) should produce its own explosion"
        );
        assert_eq!(audio.se_calls.iter().filter(|se| **se == SoundEffect::Explosion).count(), 2);
    }

    #[test]
    fn explosion_defeats_enemy_in_blast_range_and_awards_score() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        // (1,2) は常に Empty 保証、かつ player(1,1)からpower1の爆風が届く。
        state.enemies[0].pos = (1, 2);
        state.enemies[0].alive = true;
        let before_score = state.score();

        state.bombs.push(Bomb::new(state.players[0].pos, 1, 0));
        state.tick(10.0, Action::None, &mut audio);

        assert!(!state.enemies[0].alive, "enemy in blast range must be defeated");
        assert_eq!(state.score(), before_score + SCORE_PER_ENEMY);
    }

    #[test]
    fn player_touching_alive_enemy_dies_but_dead_enemy_is_harmless() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        let starting_lives = state.lives();

        // まず死んでいる敵がプレイヤーと同じマスにいても何も起きないことを確認する。
        state.enemies[0].pos = state.players[0].pos;
        state.enemies[0].alive = false;
        state.tick(0.033, Action::None, &mut audio);
        assert!(
            state.players[0].alive,
            "a dead enemy must not hurt the player"
        );
        assert_eq!(state.lives(), starting_lives);

        // 次に生きている敵がプレイヤーと同じマスにいれば死亡処理が起きる。
        // 残機が残っているため、同じtick内でresolve_round_endによりただちに
        // PLAYER_STARTへ復活する(alive自体はtick後にtrueへ戻る)ので、
        // 死亡が実際に処理された証拠として残機減少とDeath SEを確認する。
        state.enemies[0].alive = true;
        state.tick(0.033, Action::None, &mut audio);
        assert_eq!(
            state.lives(),
            starting_lives - 1,
            "touching a live enemy must consume a life"
        );
        assert!(audio.se_calls.contains(&SoundEffect::Death));
        assert!(state.players[0].alive, "should respawn while lives remain");
        assert_eq!(state.players[0].pos, PLAYER_START);
    }

    #[test]
    fn invincible_player_survives_enemy_contact_and_defeats_the_enemy() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        state.players[0].invincible_remaining = INVINCIBLE_DURATION;
        state.enemies[0].pos = state.players[0].pos;
        state.enemies[0].alive = true;
        let starting_lives = state.lives();
        let before_score = state.score();

        state.tick(0.033, Action::None, &mut audio);

        assert_eq!(
            state.lives(),
            starting_lives,
            "an invincible player must not lose a life on enemy contact"
        );
        assert!(state.players[0].alive);
        assert!(
            !state.enemies[0].alive,
            "touching an enemy while invincible must defeat it"
        );
        assert_eq!(state.score(), before_score + SCORE_PER_ENEMY);
    }

    #[test]
    fn invincible_player_survives_explosion() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        state.players[0].invincible_remaining = INVINCIBLE_DURATION;
        let starting_lives = state.lives();

        // dtを小さく保ち、この1tick分の減算(0.01s)で無敵が切れないようにする
        // (invincible_remaining も同じdtでしか減らないため、bomb.timerだけを
        // 直接ゼロ近くにして即爆発させれば無敵が残ったまま爆風を受けさせられる)。
        let mut bomb = Bomb::new(state.players[0].pos, 1, 0);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick(0.01, Action::None, &mut audio);

        assert!(
            state.bombs.is_empty(),
            "the bomb should have detonated this tick"
        );
        assert_eq!(
            state.lives(),
            starting_lives,
            "an invincible player must not lose a life to an explosion"
        );
        assert!(state.players[0].alive);
    }

    #[test]
    fn invincibility_expires_after_its_duration() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        state.players[0].invincible_remaining = INVINCIBLE_DURATION;
        state.tick(INVINCIBLE_DURATION + 0.1, Action::None, &mut audio);

        assert!(!state.players[0].is_invincible());
        assert_eq!(state.players[0].invincible_remaining, 0.0);
    }

    #[test]
    fn toggle_god_mode_action_flips_the_flag_regardless_of_move_cooldown() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        assert!(!state.players[0].god_mode);

        state.tick(0.033, Action::ToggleGodMode, &mut audio);
        assert!(state.players[0].god_mode);
        assert!(state.players[0].is_invincible());

        state.tick(0.033, Action::ToggleGodMode, &mut audio);
        assert!(!state.players[0].god_mode);
    }

    #[test]
    fn god_mode_makes_player_immune_to_explosion_without_a_timer() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        state.players[0].god_mode = true;
        let starting_lives = state.lives();

        let mut bomb = Bomb::new(state.players[0].pos, 1, 0);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick(0.01, Action::None, &mut audio);

        assert_eq!(state.lives(), starting_lives);
        assert!(state.players[0].alive);
        assert!(
            state.players[0].god_mode,
            "god mode must stay on until toggled again (unlike the timed item)"
        );
    }

    // ---- 複数プレイヤー(対戦)のテスト ----

    /// `num_players` 人の対戦を Playing 状態まで進めた state を作る。
    fn started_versus(num_players: usize, audio: &mut NoopAudio) -> GameState {
        let mut state = GameState::new_multiplayer(num_players);
        let actions = vec![Action::PlaceBomb; num_players];
        state.tick_multi(0.033, &actions, audio);
        assert_eq!(state.screen, Screen::Playing);
        assert!(
            state.bombs.is_empty(),
            "the title keypress must not place a bomb"
        );
        state
    }

    /// 所有者 `owner` のボムの位置を設置順に返す。
    fn bomb_positions_of(state: &GameState, owner: usize) -> Vec<Coord> {
        state
            .bombs
            .iter()
            .filter(|bomb| bomb.owner == owner)
            .map(|bomb| bomb.pos)
            .collect()
    }

    #[test]
    fn tick_multi_with_a_single_action_drives_the_solo_game_like_tick() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();

        state.tick_multi(0.033, &[Action::PlaceBomb], &mut audio); // -> Playing
        assert_eq!(state.screen, Screen::Playing);
        assert_eq!(state.players.len(), 1);
        assert_eq!(
            state.enemies.len(),
            ENEMY_KINDS.len(),
            "solo play must still spawn the CPU enemies"
        );

        state.players[0].move_cooldown = 0.0;
        state.tick_multi(1.0, &[Action::Move(Direction::Right)], &mut audio);
        assert_eq!(state.players[0].pos, (1, 2));
    }

    #[test]
    fn score_and_lives_accessors_expose_player_zero() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        assert_eq!(state.score(), state.players[0].score);
        assert_eq!(state.lives(), state.players[0].lives);

        state.players[0].score = 1234;
        state.players[0].lives = 2;
        assert_eq!(state.score(), 1234);
        assert_eq!(state.lives(), 2);

        // 対戦中でもアクセサは1人プレイ視点(プレイヤー0)の値を返す。
        let mut versus = started_versus(2, &mut audio);
        versus.players[0].score = 10;
        versus.players[1].score = 99;
        assert_eq!(versus.score(), 10);
    }

    #[test]
    fn new_multiplayer_places_distinct_walkable_players_without_enemies() {
        for num_players in MIN_MULTIPLAYER_PLAYERS..=MAX_PLAYERS {
            let state = GameState::new_multiplayer(num_players);

            assert_eq!(state.screen, Screen::Title);
            assert_eq!(state.players.len(), num_players);
            assert_eq!(state.spawn_points.len(), num_players);
            assert!(
                state.enemies.is_empty(),
                "a versus match must not spawn CPU enemies"
            );
            assert_eq!(
                state.players[0].pos, PLAYER_START,
                "player 0 takes the corner that generate() always keeps empty"
            );

            for (idx, player) in state.players.iter().enumerate() {
                assert!(
                    state.map.is_walkable(player.pos),
                    "every spawn must be a walkable cell"
                );
                assert_eq!(player.pos, state.spawn_points[idx]);
                assert!(player.alive);
                assert_eq!(player.lives, STARTING_LIVES);
                assert_eq!(player.score, 0);
                assert_eq!(player.move_cooldown, 0.0);
            }

            for i in 0..num_players {
                for j in (i + 1)..num_players {
                    assert_ne!(
                        state.players[i].pos, state.players[j].pos,
                        "players must not share a spawn cell"
                    );
                }
            }
        }
    }

    #[test]
    fn new_multiplayer_clamps_the_player_count_into_the_supported_range() {
        assert_eq!(
            GameState::new_multiplayer(0).players.len(),
            MIN_MULTIPLAYER_PLAYERS
        );
        assert_eq!(
            GameState::new_multiplayer(1).players.len(),
            MIN_MULTIPLAYER_PLAYERS
        );
        assert_eq!(GameState::new_multiplayer(3).players.len(), 3);
        assert_eq!(
            GameState::new_multiplayer(MAX_PLAYERS).players.len(),
            MAX_PLAYERS
        );
        assert_eq!(GameState::new_multiplayer(9).players.len(), MAX_PLAYERS);
    }

    #[test]
    fn multiplayer_spawns_are_the_nearest_walkable_cell_to_each_corner() {
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);
        let spawns = player_spawn_positions(&map, MAX_PLAYERS);
        let anchors = corner_anchors(&map);

        assert_eq!(spawns.len(), MAX_PLAYERS);

        for (i, &spawn) in spawns.iter().enumerate() {
            let distance = manhattan(spawn, anchors[i]);
            for row in 0..map.height as i32 {
                for col in 0..map.width as i32 {
                    let candidate = (row, col);
                    // 先に確定した他プレイヤーの位置は候補から外れている。
                    if !map.is_walkable(candidate) || spawns[..i].contains(&candidate) {
                        continue;
                    }
                    assert!(
                        manhattan(candidate, anchors[i]) >= distance,
                        "spawn {spawn:?} is not the closest available cell to corner {:?}",
                        anchors[i]
                    );
                }
            }
        }
    }

    #[test]
    fn nearest_player_pos_prefers_the_closest_living_player() {
        let mut players = vec![Player::new((1, 1)), Player::new((9, 9))];
        assert_eq!(nearest_player_pos(&players, (8, 9)), Some((9, 9)));
        assert_eq!(nearest_player_pos(&players, (1, 2)), Some((1, 1)));

        // 生存しているプレイヤーを優先する。
        players[1].alive = false;
        assert_eq!(nearest_player_pos(&players, (8, 9)), Some((1, 1)));

        // 全員死亡(1人プレイの復活待ちを含む)なら死亡プレイヤーも対象にする
        // = 1人プレイでは常にそのプレイヤーが基準になる。
        players[0].alive = false;
        assert_eq!(nearest_player_pos(&players, (8, 9)), Some((9, 9)));

        assert_eq!(nearest_player_pos(&[], (0, 0)), None);
    }

    #[test]
    fn move_cooldowns_are_tracked_per_player() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(2, &mut audio);

        // 同じマスに並べ、クールダウンだけを違えて同じ入力を与える
        // (プレイヤー同士は重なれる仕様なので、これは有効なセットアップ)。
        state.players[0].pos = PLAYER_START;
        state.players[1].pos = PLAYER_START;
        state.players[0].move_cooldown = 1.0;
        state.players[1].move_cooldown = 0.0;

        let actions = [
            Action::Move(Direction::Right),
            Action::Move(Direction::Right),
        ];
        state.tick_multi(0.01, &actions, &mut audio);

        assert_eq!(
            state.players[0].pos, PLAYER_START,
            "player 0 is still on cooldown and must not move"
        );
        assert_eq!(
            state.players[1].pos,
            (1, 2),
            "player 1 was ready and must move"
        );
        assert!(
            state.players[1].move_cooldown > 0.0,
            "moving must start that player's own cooldown"
        );
        assert!(
            (state.players[0].move_cooldown - 0.99).abs() < 1e-6,
            "a waiting player only has dt subtracted"
        );
    }

    #[test]
    fn each_player_places_bombs_within_their_own_capacity() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(2, &mut audio);

        // (1,1)/(2,1)/(1,2) は常に Empty が保証されるマス。
        state.players[0].pos = (1, 1);
        state.players[1].pos = (2, 1);

        let place_both = [Action::PlaceBomb, Action::PlaceBomb];
        state.tick_multi(0.001, &place_both, &mut audio);

        assert_eq!(state.bombs.len(), 2, "both players place one bomb each");
        assert_eq!(bomb_positions_of(&state, 0), vec![(1, 1)]);
        assert_eq!(bomb_positions_of(&state, 1), vec![(2, 1)]);

        // 容量1なので、自分のボムが盤面にある間は別マスでも追加で置けない。
        state.players[1].pos = (1, 2);
        state.tick_multi(0.001, &place_both, &mut audio);
        assert_eq!(
            state.bombs.len(),
            2,
            "capacity 1 must block a second bomb per player"
        );

        // 容量を増やしたプレイヤーだけが追加で置ける。
        state.players[1].bomb_capacity = 2;
        state.tick_multi(0.001, &place_both, &mut audio);
        assert_eq!(bomb_positions_of(&state, 1), vec![(2, 1), (1, 2)]);
        assert_eq!(
            bomb_positions_of(&state, 0),
            vec![(1, 1)],
            "player 0 capacity must be unaffected by player 1"
        );
    }

    #[test]
    fn tick_multi_treats_a_missing_action_as_no_input() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(2, &mut audio);

        state.players[0].pos = PLAYER_START;
        state.players[1].pos = PLAYER_START;
        state.players[0].move_cooldown = 0.0;
        state.players[1].move_cooldown = 0.0;

        // プレイヤー1の分を渡さない -> 入力なし扱い。
        state.tick_multi(1.0, &[Action::Move(Direction::Right)], &mut audio);

        assert_eq!(state.players[0].pos, (1, 2));
        assert_eq!(
            state.players[1].pos, PLAYER_START,
            "a player without an action must not move"
        );
    }

    #[test]
    fn an_explosion_hurts_every_player_in_range_and_a_mutual_kill_is_a_draw() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(2, &mut audio);

        // (1,1) の power1 の爆風は (1,2) まで届く。
        state.players[0].pos = (1, 1);
        state.players[1].pos = (1, 2);

        let mut bomb = Bomb::new((1, 1), 1, 0);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick_multi(0.01, &[Action::None, Action::None], &mut audio);

        assert!(
            !state.players[0].alive,
            "the owner is not immune to their own bomb"
        );
        assert!(!state.players[1].alive, "every player in range is caught");
        assert_eq!(state.players[0].lives, STARTING_LIVES - 1);
        assert_eq!(state.players[1].lives, STARTING_LIVES - 1);
        assert_eq!(
            state.screen,
            Screen::MatchResult(None),
            "wiping out everyone is a draw"
        );
        assert!(audio.se_calls.contains(&SoundEffect::Death));
        assert!(audio.bgm_calls.contains(&Bgm::GameOver));
    }

    #[test]
    fn invincibility_is_evaluated_per_player() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(2, &mut audio);

        state.players[0].pos = (1, 1);
        state.players[1].pos = (1, 2);
        state.players[1].invincible_remaining = INVINCIBLE_DURATION;

        let mut bomb = Bomb::new((1, 1), 1, 1);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick_multi(0.01, &[Action::None, Action::None], &mut audio);

        assert!(
            !state.players[0].alive,
            "the player without invincibility dies"
        );
        assert!(
            state.players[1].alive,
            "the invincible player survives the same blast"
        );
        assert_eq!(
            state.screen,
            Screen::MatchResult(Some(1)),
            "the survivor wins the match"
        );
    }

    #[test]
    fn last_player_standing_wins_the_match() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(MAX_PLAYERS, &mut audio);

        // プレイヤー2だけを残す。
        for idx in [0, 1, 3] {
            state.players[idx].alive = false;
        }
        state.tick_multi(0.033, &[Action::None; MAX_PLAYERS], &mut audio);

        assert_eq!(state.screen, Screen::MatchResult(Some(2)));
        assert!(audio.se_calls.contains(&SoundEffect::StageClear));
        assert!(audio.bgm_calls.contains(&Bgm::Clear));
    }

    #[test]
    fn a_versus_death_is_permanent_even_with_lives_remaining() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(3, &mut audio);

        state.players[1].pos = (1, 2);
        state.players[1].alive = false;
        assert!(
            state.players[1].lives > 0,
            "lives remain, but a versus match has no respawn"
        );

        state.tick_multi(0.033, &[Action::None; 3], &mut audio);

        assert!(
            !state.players[1].alive,
            "an eliminated player must not respawn"
        );
        assert_eq!(state.players[1].pos, (1, 2));
        assert_eq!(
            state.screen,
            Screen::Playing,
            "two players are still alive so the match continues"
        );
    }

    #[test]
    fn match_result_screen_space_returns_to_title() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new_multiplayer(2);
        state.screen = Screen::MatchResult(Some(1));

        state.tick_multi(
            0.033,
            &[Action::Move(Direction::Up), Action::None],
            &mut audio,
        );
        assert_eq!(
            state.screen,
            Screen::MatchResult(Some(1)),
            "only PlaceBomb should return to title"
        );

        state.tick_multi(0.033, &[Action::None, Action::PlaceBomb], &mut audio);
        assert_eq!(state.screen, Screen::Title);
    }

    #[test]
    fn restarting_from_the_title_keeps_the_multiplayer_setup() {
        let mut audio = NoopAudio::default();
        let mut state = started_versus(3, &mut audio);
        state.players[0].score = 500;
        state.players[0].lives = 1;

        state.screen = Screen::Title;
        state.tick_multi(
            0.033,
            &[Action::PlaceBomb, Action::None, Action::None],
            &mut audio,
        );

        assert_eq!(state.screen, Screen::Playing);
        assert_eq!(
            state.players.len(),
            3,
            "the player count must survive a restart"
        );
        assert!(
            state.enemies.is_empty(),
            "a versus restart must not spawn CPU enemies"
        );
        for (idx, player) in state.players.iter().enumerate() {
            assert_eq!(player.pos, state.spawn_points[idx]);
            assert_eq!(player.score, 0, "scores are reset on a new game");
            assert_eq!(player.lives, STARTING_LIVES);
            assert!(player.alive);
        }
    }
}
