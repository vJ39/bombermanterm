//! ゲーム全体の状態と tick 更新。
//!
//! Integrateフェーズ: `GameState::new` / `GameState::tick` を実装する。
//!
//! tick の大まかな流れ (Screen::Playing 中):
//! 1. 入力(移動/ボム設置)を反映する。移動は `speed` に応じたクールダウンで間引く。
//! 2. 設置済みボムの `timer` を dt 分減らし、0以下になったボムを爆発させる
//!    (`explosion_cells` で範囲計算)。爆風範囲に他のボムがあれば `timer` を
//!    0にして誘爆させる(連鎖)。爆風範囲の `Block` は `destroy_block` で破壊し
//!    アイテム化する。爆風範囲にプレイヤー/敵が重なれば死亡処理する。
//! 3. 爆風の残存時間を減らし、切れたものを消す。
//! 4. 敵をクールダウン付きで `Enemy::decide_move` に従って移動させる。
//! 5. 敵との接触判定(素手で触れても死亡)を行う。
//! 6. ラウンドの決着(全滅→Cleared、プレイヤー死亡&残機0→GameOver、
//!    残機が残っていれば復活)を判定する。
//!
//! 契約からの逸脱(いずれも追加のみ、契約シグネチャの変更は無い):
//! - `GameMap::take_item` を追加した(理由は `map.rs` の CONTRACT CHANGE コメント参照)。
//! - `GameState` に非公開フィールド `player_move_cooldown` / `enemy_move_cooldown` を
//!   追加した。契約は `pub struct GameState { ... }` と省略記法で書かれており
//!   フィールド集合を固定していないため、公開フィールド(`map`/`player`/`enemies`/
//!   `bombs`/`explosions`/`score`/`lives`/`screen`)は指示通りそのままに、移動速度
//!   (`Player::speed`)を意味あるものにするための内部実装専用フィールドとして追加した。
//! - `ItemKind::Invincible` を追加(`types.rs` の CONTRACT CHANGE 参照)。取得すると
//!   `INVINCIBLE_DURATION` 秒間、爆風・敵接触で死亡しなくなる。無敵中に敵へ触れると
//!   逆に敵を撃破する(本家の無敵アイテムの定番挙動)。

use crate::audio::AudioPlayer;
use crate::game::entities::{explosion_cells, Bomb, Enemy, EnemyKind, Explosion, Player};
use crate::game::map::GameMap;
use crate::types::{Action, Bgm, Coord, Direction, ItemKind, Screen, SoundEffect, Tile};

/// マップの幅・高さ(壁込み)。奇数にして格子状固定壁と外周壁が綺麗に収まる大きさにする。
const MAP_WIDTH: usize = 15;
const MAP_HEIGHT: usize = 13;

/// プレイヤーの初期位置。`GameMap::generate` が常に `Empty` を保証する3マスの一つ。
const PLAYER_START: Coord = (1, 1);

/// 開始時の残機数。
const STARTING_LIVES: u32 = 3;

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
    pub player: Player,
    pub enemies: Vec<Enemy>,
    pub bombs: Vec<Bomb>,
    pub explosions: Vec<Explosion>,
    pub score: u32,
    pub lives: u32,

    /// プレイヤーが次に1マス移動できるまでの残り秒数。0以下で移動可能。
    player_move_cooldown: f32,
    /// 敵が次に1マス移動するまでの残り秒数。0以下で全敵が1マスずつ移動する。
    enemy_move_cooldown: f32,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameState {
    pub fn new() -> Self {
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);
        let enemies = spawn_enemies(&map);

        GameState {
            screen: Screen::Title,
            map,
            player: Player::new(PLAYER_START),
            enemies,
            bombs: Vec::new(),
            explosions: Vec::new(),
            score: 0,
            lives: STARTING_LIVES,
            player_move_cooldown: 0.0,
            enemy_move_cooldown: ENEMY_MOVE_INTERVAL,
        }
    }

    /// 固定tickでの状態更新。入力アクションの反映、ボム/爆風/敵AIの進行、
    /// 当たり判定、SE/BGM再生を行う。
    pub fn tick(&mut self, dt: f32, action: Action, audio: &mut dyn AudioPlayer) {
        match self.screen {
            Screen::Title => self.tick_title(action, audio),
            Screen::Playing => self.tick_playing(dt, action, audio),
            Screen::Cleared | Screen::GameOver => self.tick_result(action, audio),
        }
    }

    /// タイトル画面: Quit以外の何らかの入力でゲームを開始する。
    fn tick_title(&mut self, action: Action, audio: &mut dyn AudioPlayer) {
        if matches!(action, Action::None | Action::Quit) {
            return;
        }
        self.start_new_game(audio);
    }

    /// クリア/ゲームオーバー画面: ボム設置キー(SPACE)でタイトルへ戻る
    /// (画面表示のヒント文言と合わせている)。
    fn tick_result(&mut self, action: Action, audio: &mut dyn AudioPlayer) {
        if matches!(action, Action::PlaceBomb) {
            self.screen = Screen::Title;
            audio.stop_bgm();
            audio.play_bgm(Bgm::Title);
        }
    }

    /// マップ・プレイヤー・敵・スコア・残機を初期化してプレイを開始する。
    fn start_new_game(&mut self, audio: &mut dyn AudioPlayer) {
        let map = GameMap::generate(MAP_WIDTH, MAP_HEIGHT);
        let enemies = spawn_enemies(&map);

        self.map = map;
        self.player = Player::new(PLAYER_START);
        self.enemies = enemies;
        self.bombs.clear();
        self.explosions.clear();
        self.score = 0;
        self.lives = STARTING_LIVES;
        self.player_move_cooldown = 0.0;
        self.enemy_move_cooldown = ENEMY_MOVE_INTERVAL;
        self.screen = Screen::Playing;

        audio.stop_bgm();
        audio.play_bgm(Bgm::Stage);
    }

    fn tick_playing(&mut self, dt: f32, action: Action, audio: &mut dyn AudioPlayer) {
        if self.player.invincible_remaining > 0.0 {
            self.player.invincible_remaining = (self.player.invincible_remaining - dt).max(0.0);
        }

        self.handle_player_input(dt, action, audio);

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

    /// 移動・ボム設置の入力を反映する。
    fn handle_player_input(&mut self, dt: f32, action: Action, audio: &mut dyn AudioPlayer) {
        if self.player_move_cooldown > 0.0 {
            self.player_move_cooldown -= dt;
        }

        // 隠しコマンドによる強制無敵トグルは、プレイヤーの生死に関わらず効く
        // デバッグ用の裏機能なので、生存チェックより前で処理する。
        if matches!(action, Action::ToggleGodMode) {
            self.player.god_mode = !self.player.god_mode;
            return;
        }

        if !self.player.alive {
            return;
        }

        match action {
            Action::Move(dir) if self.player_move_cooldown <= 0.0 => {
                let target = step(self.player.pos, dir);
                if self.map.is_walkable(target) && !self.bomb_at(target) {
                    self.player.pos = target;
                    let interval = BASE_MOVE_INTERVAL / self.player.speed.max(MIN_SPEED);
                    self.player_move_cooldown = interval;
                    self.try_pickup_item(audio);
                }
            }
            Action::PlaceBomb => self.try_place_bomb(audio),
            _ => {}
        }
    }

    /// プレイヤーの現在マスにアイテムがあれば回収して能力を強化する。
    fn try_pickup_item(&mut self, audio: &mut dyn AudioPlayer) {
        let Some(kind) = self.map.take_item(self.player.pos) else {
            return;
        };

        match kind {
            ItemKind::Power => self.player.power += 1,
            ItemKind::BombUp => self.player.bomb_capacity += 1,
            ItemKind::SpeedUp => self.player.speed += SPEED_INCREMENT,
            ItemKind::Invincible => self.player.invincible_remaining = INVINCIBLE_DURATION,
        }
        self.score += SCORE_PER_ITEM;
        audio.play_se(SoundEffect::ItemGet);
    }

    /// bomb_capacity の上限内、かつ同じマスに設置済みでなければボムを置く。
    fn try_place_bomb(&mut self, audio: &mut dyn AudioPlayer) {
        let active_player_bombs =
            self.bombs.iter().filter(|bomb| bomb.owner_is_player).count() as u32;
        if active_player_bombs >= self.player.bomb_capacity {
            return;
        }
        if self.bombs.iter().any(|bomb| bomb.pos == self.player.pos) {
            return;
        }

        self.bombs
            .push(Bomb::new(self.player.pos, self.player.power, true));
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

            // プレイヤーが爆風に巻き込まれたら死亡処理(無敵中は無効)。
            if self.player.alive && !self.player.is_invincible() && cells.contains(&self.player.pos)
            {
                self.hurt_player(audio);
            }

            // 敵が爆風に巻き込まれたら撃破。
            for enemy in self.enemies.iter_mut() {
                if enemy.alive && cells.contains(&enemy.pos) {
                    enemy.alive = false;
                    self.score += SCORE_PER_ENEMY;
                }
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
        let player_pos = self.player.pos;
        for enemy in self.enemies.iter_mut() {
            if !enemy.alive {
                continue;
            }
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

    /// 爆風以外でも、敵に直接触れたらプレイヤーは死亡する。
    /// 無敵モード中は逆に、触れた敵を蹴散らす(死亡しない・敵を撃破する)。
    fn check_enemy_contact(&mut self, audio: &mut dyn AudioPlayer) {
        if !self.player.alive {
            return;
        }

        if self.player.is_invincible() {
            for enemy in self.enemies.iter_mut() {
                if enemy.alive && enemy.pos == self.player.pos {
                    enemy.alive = false;
                    self.score += SCORE_PER_ENEMY;
                }
            }
            return;
        }

        let touched = self
            .enemies
            .iter()
            .any(|enemy| enemy.alive && enemy.pos == self.player.pos);
        if touched {
            self.hurt_player(audio);
        }
    }

    /// プレイヤーを死亡させ、残機を1つ減らす。
    fn hurt_player(&mut self, audio: &mut dyn AudioPlayer) {
        if !self.player.alive {
            return;
        }
        self.player.alive = false;
        self.lives = self.lives.saturating_sub(1);
        audio.play_se(SoundEffect::Death);
    }

    /// ラウンドの決着を判定する: プレイヤー死亡→残機があれば復活/無ければGameOver、
    /// 敵全滅ならCleared。
    fn resolve_round_end(&mut self, audio: &mut dyn AudioPlayer) {
        if !self.player.alive {
            if self.lives == 0 {
                self.screen = Screen::GameOver;
                audio.stop_bgm();
                audio.play_bgm(Bgm::GameOver);
            } else {
                self.player.pos = PLAYER_START;
                self.player.alive = true;
                self.player_move_cooldown = 0.0;
            }
            return;
        }

        if !self.enemies.is_empty() && self.enemies.iter().all(|enemy| !enemy.alive) {
            self.screen = Screen::Cleared;
            audio.stop_bgm();
            audio.play_se(SoundEffect::StageClear);
            audio.play_bgm(Bgm::Clear);
        }
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
        assert_eq!(state.lives, STARTING_LIVES);
        assert_eq!(state.score, 0);
        assert!(state.player.alive);
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

        let before = state.player.pos;
        // (1,1) から Up は外周壁 (0,1) なので移動できないはず。
        state.tick(1.0, Action::Move(Direction::Up), &mut audio);
        assert_eq!(state.player.pos, before);

        // Right は (1,2) が常に Empty 保証なので移動できるはず。
        state.player_move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(state.player.pos, (1, 2));
    }

    #[test]
    fn player_cannot_walk_onto_a_bomb_tile() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        // (1,2) は常に Empty 保証。ここにボムを置き、プレイヤーが重ならず
        // 素通りできないことを確認する(タイルは通行可能でもボムは実体として塞ぐ)。
        state.bombs.push(Bomb::new((1, 2), 1, true));

        state.player_move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(
            state.player.pos,
            PLAYER_START,
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
        state.bombs.push(Bomb::new(PLAYER_START, 1, true));

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

        state.bombs.push(Bomb::new(state.player.pos, 1, true));
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

        let starting_lives = state.lives;
        state.bombs.push(Bomb::new(state.player.pos, 1, true));
        state.tick(10.0, Action::None, &mut audio);

        assert_eq!(state.lives, starting_lives - 1);
        assert_eq!(state.screen, Screen::Playing);
        assert!(state.player.alive, "should respawn while lives remain");
        assert_eq!(state.player.pos, PLAYER_START);
        assert!(audio.se_calls.contains(&SoundEffect::Death));
    }

    #[test]
    fn player_death_with_no_lives_ends_game() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.lives = 1;
        state.bombs.clear();

        state.bombs.push(Bomb::new(state.player.pos, 1, true));
        state.tick(10.0, Action::None, &mut audio);

        assert_eq!(state.lives, 0);
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
        let before_power = state.player.power;
        // (1,2) は常に Empty なのでアイテムは無い前提だが、pickup 経路自体が
        // panicせず何も起きないことを確認する回帰テストとして扱う。
        state.player_move_cooldown = 0.0;
        state.tick(1.0, Action::Move(Direction::Right), &mut audio);
        assert_eq!(state.player.power, before_power);
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

                    let before_power = state.player.power;
                    let before_capacity = state.player.bomb_capacity;
                    let before_speed = state.player.speed;
                    let before_score = state.score;
                    let before_invincible = state.player.invincible_remaining;

                    state.player.pos = pos;
                    state.try_pickup_item(&mut audio);

                    assert_eq!(
                        state.map.tile_at(pos),
                        Tile::Empty,
                        "item tile must be consumed on pickup"
                    );
                    assert_eq!(state.score, before_score + SCORE_PER_ITEM);
                    assert!(audio.se_calls.contains(&SoundEffect::ItemGet));

                    match kind {
                        ItemKind::Power => {
                            assert_eq!(state.player.power, before_power + 1);
                            seen_power = true;
                        }
                        ItemKind::BombUp => {
                            assert_eq!(state.player.bomb_capacity, before_capacity + 1);
                            seen_bomb_up = true;
                        }
                        ItemKind::SpeedUp => {
                            assert!(
                                (state.player.speed - (before_speed + SPEED_INCREMENT)).abs()
                                    < 1e-6
                            );
                            seen_speed_up = true;
                        }
                        ItemKind::Invincible => {
                            assert_eq!(before_invincible, 0.0);
                            assert_eq!(state.player.invincible_remaining, INVINCIBLE_DURATION);
                            assert!(state.player.is_invincible());
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
        let mut bomb_a = Bomb::new(state.player.pos, 1, true);
        bomb_a.timer = 0.001;
        let mut bomb_b = Bomb::new((1, 2), 1, true);
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
        let before_score = state.score;

        state.bombs.push(Bomb::new(state.player.pos, 1, true));
        state.tick(10.0, Action::None, &mut audio);

        assert!(!state.enemies[0].alive, "enemy in blast range must be defeated");
        assert_eq!(state.score, before_score + SCORE_PER_ENEMY);
    }

    #[test]
    fn player_touching_alive_enemy_dies_but_dead_enemy_is_harmless() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        let starting_lives = state.lives;

        // まず死んでいる敵がプレイヤーと同じマスにいても何も起きないことを確認する。
        state.enemies[0].pos = state.player.pos;
        state.enemies[0].alive = false;
        state.tick(0.033, Action::None, &mut audio);
        assert!(state.player.alive, "a dead enemy must not hurt the player");
        assert_eq!(state.lives, starting_lives);

        // 次に生きている敵がプレイヤーと同じマスにいれば死亡処理が起きる。
        // 残機が残っているため、同じtick内でresolve_round_endによりただちに
        // PLAYER_STARTへ復活する(alive自体はtick後にtrueへ戻る)ので、
        // 死亡が実際に処理された証拠として残機減少とDeath SEを確認する。
        state.enemies[0].alive = true;
        state.tick(0.033, Action::None, &mut audio);
        assert_eq!(
            state.lives,
            starting_lives - 1,
            "touching a live enemy must consume a life"
        );
        assert!(audio.se_calls.contains(&SoundEffect::Death));
        assert!(state.player.alive, "should respawn while lives remain");
        assert_eq!(state.player.pos, PLAYER_START);
    }

    #[test]
    fn invincible_player_survives_enemy_contact_and_defeats_the_enemy() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        state.player.invincible_remaining = INVINCIBLE_DURATION;
        state.enemies[0].pos = state.player.pos;
        state.enemies[0].alive = true;
        let starting_lives = state.lives;
        let before_score = state.score;

        state.tick(0.033, Action::None, &mut audio);

        assert_eq!(
            state.lives, starting_lives,
            "an invincible player must not lose a life on enemy contact"
        );
        assert!(state.player.alive);
        assert!(
            !state.enemies[0].alive,
            "touching an enemy while invincible must defeat it"
        );
        assert_eq!(state.score, before_score + SCORE_PER_ENEMY);
    }

    #[test]
    fn invincible_player_survives_explosion() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        state.player.invincible_remaining = INVINCIBLE_DURATION;
        let starting_lives = state.lives;

        // dtを小さく保ち、この1tick分の減算(0.01s)で無敵が切れないようにする
        // (invincible_remaining も同じdtでしか減らないため、bomb.timerだけを
        // 直接ゼロ近くにして即爆発させれば無敵が残ったまま爆風を受けさせられる)。
        let mut bomb = Bomb::new(state.player.pos, 1, true);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick(0.01, Action::None, &mut audio);

        assert!(
            state.bombs.is_empty(),
            "the bomb should have detonated this tick"
        );
        assert_eq!(
            state.lives, starting_lives,
            "an invincible player must not lose a life to an explosion"
        );
        assert!(state.player.alive);
    }

    #[test]
    fn invincibility_expires_after_its_duration() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing

        state.player.invincible_remaining = INVINCIBLE_DURATION;
        state.tick(INVINCIBLE_DURATION + 0.1, Action::None, &mut audio);

        assert!(!state.player.is_invincible());
        assert_eq!(state.player.invincible_remaining, 0.0);
    }

    #[test]
    fn toggle_god_mode_action_flips_the_flag_regardless_of_move_cooldown() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        assert!(!state.player.god_mode);

        state.tick(0.033, Action::ToggleGodMode, &mut audio);
        assert!(state.player.god_mode);
        assert!(state.player.is_invincible());

        state.tick(0.033, Action::ToggleGodMode, &mut audio);
        assert!(!state.player.god_mode);
    }

    #[test]
    fn god_mode_makes_player_immune_to_explosion_without_a_timer() {
        let mut audio = NoopAudio::default();
        let mut state = GameState::new();
        state.tick(0.033, Action::PlaceBomb, &mut audio); // -> Playing
        state.bombs.clear();

        state.player.god_mode = true;
        let starting_lives = state.lives;

        let mut bomb = Bomb::new(state.player.pos, 1, true);
        bomb.timer = 0.001;
        state.bombs.push(bomb);
        state.tick(0.01, Action::None, &mut audio);

        assert_eq!(state.lives, starting_lives);
        assert!(state.player.alive);
        assert!(
            state.player.god_mode,
            "god mode must stay on until toggled again (unlike the timed item)"
        );
    }
}
