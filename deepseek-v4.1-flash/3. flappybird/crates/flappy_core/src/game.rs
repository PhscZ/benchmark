//! Gameplay: state machine, fixed-timestep physics, collision, scoring and
//! input arbitration. Platform-free so the native and wasm builds run the exact
//! same rules.
//!
//! # Coordinate system
//!
//! Everything here works in *logical* pixels inside a fixed `360x640` play
//! area. Backends scale that area to the window/canvas and letterbox the rest,
//! which is what keeps difficulty identical on every device and window size.
//!
//! # Determinism / frame-rate independence
//!
//! Real frame deltas are accumulated and consumed in fixed `1/120 s` substeps
//! (bounded per frame). Physics, spawning, scoring and collision therefore
//! produce identical results at 30, 60, 144 or stuttering frame rates, and a
//! long stall can never teleport the bird through a pipe.

use crate::audio::{Sound, SoundSet};
use crate::rng::Rng;
use crate::settings::Settings;

// ---------------------------------------------------------------------------
// Geometry (all in logical pixels)
// ---------------------------------------------------------------------------

pub const LOGICAL_W: i32 = 360;
pub const LOGICAL_H: i32 = 640;
pub const GROUND_H: i32 = 96;
/// Height of the playfield above the ground.
pub const PLAY_H: i32 = LOGICAL_H - GROUND_H;

pub const BIRD_X: f32 = 96.0;
/// Half-size of the *drawn* bird body (an ellipse).
pub const BIRD_HALF_W: f32 = 12.0;
pub const BIRD_HALF_H: f32 = 9.0;
/// Half-size of the collision box. Deliberately 2px inside the drawn body on
/// each axis: the box is rotation-independent, so hits are fair and repeatable
/// while still matching what the player sees.
pub const BIRD_HIT_HALF_W: f32 = 10.0;
pub const BIRD_HIT_HALF_H: f32 = 7.0;

pub const PIPE_W: i32 = 62;
pub const PIPE_CAP_H: i32 = 26;
/// The cap sticks out this far past the pipe body on both sides.
pub const PIPE_CAP_OVERHANG: i32 = 6;
pub const PIPE_GAP: i32 = 150;
pub const PIPE_SPACING: f32 = 210.0;
/// Head start before the first pipe reaches the bird.
pub const FIRST_PIPE_OFFSET: f32 = 130.0;
/// Hard upper bound on live pipes; off-screen ones are dropped, so memory is
/// O(1) no matter how long a run lasts.
pub const MAX_PIPES: usize = 6;
/// Distance kept between a gap edge and the playfield ceiling/ground, which is
/// what makes every generated gap reachable.
pub const GAP_EDGE_MARGIN: i32 = 64;
/// Largest vertical step between consecutive gap centres.
pub const MAX_GAP_DELTA: i32 = 128;

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

pub const GRAVITY: f32 = 1500.0;
pub const FLAP_IMPULSE: f32 = -430.0;
pub const MAX_FALL_SPEED: f32 = 720.0;
pub const BASE_SPEED: f32 = 148.0;
pub const SPEED_PER_POINT: f32 = 2.0;
pub const MAX_SPEED: f32 = 232.0;

pub const FIXED_DT: f32 = 1.0 / 120.0;
/// At most this many substeps are simulated per frame; the remainder of a long
/// stall is discarded rather than fast-forwarding the world.
pub const MAX_SUBSTEPS: u32 = 8;
/// Hard clamp on a single frame's delta.
pub const MAX_FRAME_DT: f32 = 0.25;

pub const READY_BIRD_Y: f32 = 258.0;
/// Input is ignored for this long after dying, so the fatal tap cannot restart.
pub const RESTART_LOCK: f32 = 0.35;
/// Game-over panel appears once the bird has landed or this much time passed.
pub const PANEL_DELAY: f32 = 0.55;

pub const BUTTON_SIZE: i32 = 44;
pub const BUTTON_MARGIN: i32 = 12;
pub const BUTTON_GAP: i32 = 8;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Title screen; the bird hovers and waits for the first flap.
    Ready,
    Playing,
    Paused,
    GameOver,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// Space / ArrowUp / W
    Flap,
    /// P / Escape
    Pause,
    /// R / Enter (game over only)
    Restart,
    /// M
    Mute,
}

#[derive(Clone, Copy, Debug)]
pub enum Input {
    PointerDown { x: f32, y: f32 },
    PointerUp { x: f32, y: f32 },
    PointerMove { x: f32, y: f32 },
    KeyDown(Key),
    KeyUp(Key),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Pause,
    Mute,
    Restart,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[inline]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && y >= self.y as f32
            && x < (self.x + self.w) as f32
            && y < (self.y + self.h) as f32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Pipe {
    /// Left edge of the pipe body.
    pub x: f32,
    /// Centre of the gap.
    pub gap_y: i32,
    pub scored: bool,
}

impl Pipe {
    #[inline]
    pub fn gap_top(&self) -> i32 {
        self.gap_y - PIPE_GAP / 2
    }

    #[inline]
    pub fn gap_bottom(&self) -> i32 {
        self.gap_y + PIPE_GAP / 2
    }

    /// Collision rectangles, identical to the rectangles the renderer fills:
    /// upper body, lower body, and the two caps (which overhang the body).
    pub fn colliders(&self) -> [(f32, f32, f32, f32); 4] {
        let x = self.x;
        let w = PIPE_W as f32;
        let cap_x = x - PIPE_CAP_OVERHANG as f32;
        let cap_w = w + 2.0 * PIPE_CAP_OVERHANG as f32;
        let gt = self.gap_top();
        let gb = self.gap_bottom();
        [
            (x, 0.0, w, gt as f32),
            (x, gb as f32, w, PLAY_H as f32 - gb as f32),
            (cap_x, (gt - PIPE_CAP_H) as f32, cap_w, PIPE_CAP_H as f32),
            (cap_x, gb as f32, cap_w, PIPE_CAP_H as f32),
        ]
    }
}

// ---------------------------------------------------------------------------
// Game
// ---------------------------------------------------------------------------

pub struct Game {
    pub(crate) rng: Rng,
    pub(crate) phase: Phase,
    pub(crate) bird_y: f32,
    pub(crate) bird_vy: f32,
    pub(crate) bird_rot: f32,
    pub(crate) flap_anim: f32,
    pub(crate) wing_t: f32,
    pub(crate) pipes: Vec<Pipe>,
    pub(crate) score: u32,
    pub(crate) new_best: bool,
    pub(crate) speed: f32,
    pub(crate) ground_scroll: f32,
    pub(crate) hill_far: f32,
    pub(crate) hill_near: f32,
    pub(crate) cloud_scroll: f32,
    pub(crate) time: f32,
    pub(crate) acc: f32,
    pub(crate) held: u8,
    pub(crate) pointer: (f32, f32),
    pub(crate) restart_lock: f32,
    pub(crate) death_t: f32,
    pub(crate) landed: bool,
    pub(crate) panel_t: f32,
    pub(crate) flash: f32,
    pub(crate) suspended: bool,
    pub(crate) paused_by_focus: bool,
    pub(crate) settings: Settings,
    settings_dirty: bool,
}

impl Game {
    pub fn new(seed: u64) -> Self {
        Game {
            rng: Rng::new(seed),
            phase: Phase::Ready,
            bird_y: READY_BIRD_Y,
            bird_vy: 0.0,
            bird_rot: 0.0,
            flap_anim: f32::INFINITY,
            wing_t: 0.0,
            pipes: Vec::with_capacity(MAX_PIPES),
            score: 0,
            new_best: false,
            speed: BASE_SPEED,
            ground_scroll: 0.0,
            hill_far: 0.0,
            hill_near: 0.0,
            cloud_scroll: 0.0,
            time: 0.0,
            acc: 0.0,
            held: 0,
            pointer: (-1.0, -1.0),
            restart_lock: RESTART_LOCK,
            death_t: 0.0,
            landed: false,
            panel_t: 0.0,
            flash: 0.0,
            suspended: false,
            paused_by_focus: false,
            settings: Settings::default(),
            settings_dirty: false,
        }
    }

    // -- accessors ---------------------------------------------------------

    #[inline]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    #[inline]
    pub fn score(&self) -> u32 {
        self.score
    }

    #[inline]
    pub fn best(&self) -> u32 {
        self.settings.best
    }

    #[inline]
    pub fn muted(&self) -> bool {
        self.settings.muted
    }

    #[inline]
    pub fn settings(&self) -> Settings {
        self.settings
    }

    #[inline]
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    #[inline]
    pub fn paused_by_focus(&self) -> bool {
        self.paused_by_focus
    }

    #[inline]
    pub fn pipes(&self) -> &[Pipe] {
        &self.pipes
    }

    /// Axis-aligned collision box of the bird, in logical pixels.
    #[inline]
    pub fn bird_hitbox(&self) -> (f32, f32, f32, f32) {
        (
            BIRD_X - BIRD_HIT_HALF_W,
            self.bird_y - BIRD_HIT_HALF_H,
            BIRD_X + BIRD_HIT_HALF_W,
            self.bird_y + BIRD_HIT_HALF_H,
        )
    }

    /// Where the bird is drawn this frame (the ready screen bobs it).
    pub fn bird_draw_y(&self) -> f32 {
        if self.phase == Phase::Ready {
            READY_BIRD_Y + (self.time * 3.2).sin() * 7.0
        } else {
            self.bird_y
        }
    }

    /// Call once after construction with whatever the platform loaded.
    pub fn apply_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.settings_dirty = false;
    }

    /// Returns `true` (once) when the caller should persist the settings.
    pub fn take_settings_dirty(&mut self) -> bool {
        let d = self.settings_dirty;
        self.settings_dirty = false;
        d
    }

    // -- UI layout ---------------------------------------------------------

    pub fn button_rect(&self, b: Button) -> Rect {
        let mute = Rect {
            x: LOGICAL_W - BUTTON_MARGIN - BUTTON_SIZE,
            y: BUTTON_MARGIN,
            w: BUTTON_SIZE,
            h: BUTTON_SIZE,
        };
        match b {
            Button::Mute => mute,
            Button::Pause => Rect {
                x: mute.x - BUTTON_GAP - BUTTON_SIZE,
                y: BUTTON_MARGIN,
                w: BUTTON_SIZE,
                h: BUTTON_SIZE,
            },
            Button::Restart => Rect {
                x: (LOGICAL_W - 184) / 2,
                y: 396,
                w: 184,
                h: 56,
            },
        }
    }

    /// The pause button only exists where pausing is meaningful.
    pub fn pause_button_visible(&self) -> bool {
        matches!(self.phase, Phase::Playing | Phase::Paused)
    }

    pub fn panel_visible(&self) -> bool {
        self.phase == Phase::GameOver && (self.landed || self.death_t >= PANEL_DELAY)
    }

    pub fn restart_button_visible(&self) -> bool {
        self.panel_visible()
    }

    pub fn can_restart(&self) -> bool {
        self.panel_visible() && self.restart_lock <= 0.0
    }

    /// Which button (if any) is under a logical-space point.
    pub fn button_at(&self, x: f32, y: f32) -> Option<Button> {
        if self.pause_button_visible() && self.button_rect(Button::Pause).contains(x, y) {
            return Some(Button::Pause);
        }
        if self.button_rect(Button::Mute).contains(x, y) {
            return Some(Button::Mute);
        }
        if self.restart_button_visible() && self.button_rect(Button::Restart).contains(x, y) {
            return Some(Button::Restart);
        }
        None
    }

    // -- input -------------------------------------------------------------

    /// Feeds one platform event. Returns the sounds it should trigger.
    ///
    /// Arbitration rules enforced here:
    /// * buttons are hit-tested first, so pressing one never also flaps;
    /// * a key that is already held produces nothing (kills OS key-repeat and
    ///   duplicate key/mouse deliveries);
    /// * taps are consumed by state transitions (resume, restart) instead of
    ///   leaking through as a flap.
    pub fn input(&mut self, ev: Input) -> SoundSet {
        let mut out = SoundSet::EMPTY;
        match ev {
            Input::PointerMove { x, y } => self.pointer = (x, y),
            Input::PointerUp { x, y } => self.pointer = (x, y),
            Input::PointerDown { x, y } => {
                self.pointer = (x, y);
                match self.button_at(x, y) {
                    Some(Button::Mute) => self.toggle_mute(&mut out),
                    Some(Button::Pause) => self.toggle_pause(&mut out),
                    Some(Button::Restart) => self.restart(&mut out),
                    None => self.primary_action(&mut out),
                }
            }
            Input::KeyDown(k) => {
                // Ignore auto-repeat / duplicate key-down for a held key.
                if self.set_held(k, true) {
                    match k {
                        Key::Flap => self.primary_action(&mut out),
                        Key::Pause => self.toggle_pause(&mut out),
                        Key::Mute => self.toggle_mute(&mut out),
                        Key::Restart => {
                            if self.phase == Phase::GameOver && self.can_restart() {
                                self.restart(&mut out);
                            }
                        }
                    }
                }
            }
            Input::KeyUp(k) => {
                self.set_held(k, false);
            }
        }
        out
    }

    /// Drops all held-key state. Backends call this when the window/tab loses
    /// focus, because the matching key-up will never arrive.
    pub fn clear_held(&mut self) {
        self.held = 0;
    }

    #[inline]
    fn set_held(&mut self, k: Key, down: bool) -> bool {
        let bit = 1u8 << (k as u8);
        let was = self.held & bit != 0;
        if down {
            self.held |= bit;
        } else {
            self.held &= !bit;
        }
        down && !was
    }

    /// A tap/press that is not on a button: flap, start, resume or restart
    /// depending on the state. Exactly one of those happens per event.
    fn primary_action(&mut self, out: &mut SoundSet) {
        match self.phase {
            Phase::Ready => self.start(out),
            Phase::Playing => self.flap(out),
            // Resuming is explicit: this tap is spent on the resume.
            Phase::Paused => self.toggle_pause(out),
            Phase::GameOver => {
                if self.can_restart() {
                    self.restart(out);
                }
            }
        }
    }

    // -- state transitions -------------------------------------------------

    fn flap(&mut self, out: &mut SoundSet) {
        self.bird_vy = FLAP_IMPULSE;
        self.flap_anim = 0.0;
        out.insert(Sound::Flap);
    }

    fn start(&mut self, out: &mut SoundSet) {
        self.phase = Phase::Playing;
        self.score = 0;
        self.new_best = false;
        self.speed = BASE_SPEED;
        self.acc = 0.0;
        self.death_t = 0.0;
        self.landed = false;
        self.panel_t = 0.0;
        self.bird_y = READY_BIRD_Y;
        self.bird_rot = 0.0;
        self.pipes.clear();
        let x = LOGICAL_W as f32 + FIRST_PIPE_OFFSET;
        self.spawn_pipe(x);
        self.flap(out);
    }

    fn die(&mut self, out: &mut SoundSet) {
        self.phase = Phase::GameOver;
        self.new_best = self.settings.record_score(self.score);
        if self.new_best {
            self.settings_dirty = true;
        }
        self.death_t = 0.0;
        self.landed = false;
        self.panel_t = 0.0;
        self.restart_lock = RESTART_LOCK;
        self.flash = 1.0;
        self.acc = 0.0;
        out.insert(Sound::Hit);
    }

    /// Full reset back to the ready screen. Never touches the process, the
    /// window or the page: only gameplay state.
    fn restart(&mut self, out: &mut SoundSet) {
        self.phase = Phase::Ready;
        self.pipes.clear();
        self.score = 0;
        self.new_best = false;
        self.speed = BASE_SPEED;
        self.bird_y = READY_BIRD_Y;
        self.bird_vy = 0.0;
        self.bird_rot = 0.0;
        self.flap_anim = f32::INFINITY;
        self.acc = 0.0;
        self.death_t = 0.0;
        self.landed = false;
        self.panel_t = 0.0;
        self.flash = 0.0;
        self.restart_lock = RESTART_LOCK;
        out.insert(Sound::Click);
    }

    fn toggle_pause(&mut self, out: &mut SoundSet) {
        match self.phase {
            Phase::Playing => {
                self.phase = Phase::Paused;
                self.paused_by_focus = false;
                self.acc = 0.0;
                out.insert(Sound::Click);
            }
            Phase::Paused => {
                self.phase = Phase::Playing;
                self.paused_by_focus = false;
                // Never carry time across the pause.
                self.acc = 0.0;
                out.insert(Sound::Click);
            }
            _ => {}
        }
    }

    fn toggle_mute(&mut self, out: &mut SoundSet) {
        self.settings.muted = !self.settings.muted;
        self.settings_dirty = true;
        if !self.settings.muted {
            out.insert(Sound::Click);
        }
    }

    /// Window focus / tab visibility changed.
    ///
    /// Losing focus pauses a running game and *never* auto-resumes: the player
    /// must ask for it. Any accumulated time is dropped so returning to the tab
    /// cannot apply a large physics step.
    pub fn set_suspended(&mut self, suspended: bool) {
        if suspended {
            if self.phase == Phase::Playing {
                self.phase = Phase::Paused;
                self.paused_by_focus = true;
            }
        }
        self.suspended = suspended;
        self.acc = 0.0;
        self.clear_held();
    }

    // -- simulation --------------------------------------------------------

    /// Advances the world by `dt` seconds of real time.
    pub fn advance(&mut self, dt: f32) -> SoundSet {
        let mut out = SoundSet::EMPTY;
        if self.suspended {
            self.acc = 0.0;
            return out;
        }
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_FRAME_DT) } else { 0.0 };

        self.time += dt;
        self.restart_lock = (self.restart_lock - dt).max(0.0);
        self.flash = (self.flash - dt * 5.0).max(0.0);

        if self.phase == Phase::GameOver {
            self.death_t += dt;
        }
        let target = if self.panel_visible() { 1.0 } else { 0.0 };
        // Frame-rate independent exponential approach.
        self.panel_t += (target - self.panel_t) * (1.0 - (-12.0 * dt).exp());
        if (self.panel_t - target).abs() < 0.002 {
            self.panel_t = target;
        }

        if self.phase != Phase::Paused {
            // Purely cosmetic parallax; the world only scrolls in `step`.
            self.cloud_scroll += 7.0 * dt;
            self.wing_t += dt;
            if self.phase == Phase::Ready {
                self.flap_anim += dt;
            }
        }

        let stepping = self.phase == Phase::Playing
            || (self.phase == Phase::GameOver && !self.landed);
        if !stepping {
            self.acc = 0.0;
            return out;
        }

        self.acc += dt;
        let mut steps = 0u32;
        while self.acc >= FIXED_DT {
            self.acc -= FIXED_DT;
            self.step(FIXED_DT, &mut out);
            steps += 1;
            if steps >= MAX_SUBSTEPS {
                // Stalled far longer than we are willing to simulate: drop the
                // backlog instead of fast-forwarding.
                self.acc = 0.0;
                break;
            }
            let still = self.phase == Phase::Playing
                || (self.phase == Phase::GameOver && !self.landed);
            if !still {
                self.acc = 0.0;
                break;
            }
        }
        out
    }

    /// One fixed physics substep.
    fn step(&mut self, dt: f32, out: &mut SoundSet) {
        match self.phase {
            Phase::Playing => {
                self.bird_vy = (self.bird_vy + GRAVITY * dt).min(MAX_FALL_SPEED);
                self.bird_y += self.bird_vy * dt;
                self.update_rotation(dt);
                self.flap_anim += dt;

                let dx = self.speed * dt;
                self.ground_scroll += dx;
                self.hill_far += dx * 0.16;
                self.hill_near += dx * 0.40;
                self.cloud_scroll += dx * 0.22;

                self.advance_pipes(dx);
                self.check_score(out);
                self.check_collisions(out);
            }
            Phase::GameOver if !self.landed => {
                self.bird_vy = (self.bird_vy + GRAVITY * dt).min(MAX_FALL_SPEED);
                self.bird_y += self.bird_vy * dt;
                self.bird_rot = (self.bird_rot + 3.2 * dt).min(1.55);
                if self.bird_y + BIRD_HIT_HALF_H >= PLAY_H as f32 {
                    self.bird_y = PLAY_H as f32 - BIRD_HALF_H;
                    self.landed = true;
                }
            }
            _ => {}
        }
    }

    fn update_rotation(&mut self, dt: f32) {
        let target = if self.bird_vy < 0.0 {
            -0.42
        } else {
            (self.bird_vy / MAX_FALL_SPEED) * 1.15
        };
        self.bird_rot += (target - self.bird_rot) * (1.0 - (-16.0 * dt).exp());
    }

    /// Moves pipes left, drops off-screen ones, spawns new ones.
    fn advance_pipes(&mut self, dx: f32) {
        for p in self.pipes.iter_mut() {
            p.x -= dx;
        }
        // Recycle: anything fully past the left edge is gone for good.
        let mut drop = 0;
        while drop < self.pipes.len()
            && self.pipes[drop].x + ((PIPE_W + PIPE_CAP_OVERHANG) as f32) < 0.0
        {
            drop += 1;
        }
        if drop > 0 {
            self.pipes.drain(..drop);
        }
        let last_x = self.pipes.last().map(|p| p.x).unwrap_or(f32::NEG_INFINITY);
        if self.pipes.len() < MAX_PIPES && last_x <= LOGICAL_W as f32 - PIPE_SPACING {
            let x = LOGICAL_W as f32;
            self.spawn_pipe(x);
        }
    }

    fn spawn_pipe(&mut self, x: f32) {
        let min_center = GAP_EDGE_MARGIN + PIPE_GAP / 2;
        let max_center = PLAY_H - GAP_EDGE_MARGIN - PIPE_GAP / 2;
        let mut gap_y = self.rng.range_i32(min_center, max_center);
        if let Some(prev) = self.pipes.last().map(|p| p.gap_y) {
            // Randomised, but never a jump the player cannot make.
            let lo = (prev - MAX_GAP_DELTA).max(min_center);
            let hi = (prev + MAX_GAP_DELTA).min(max_center);
            gap_y = self.rng.range_i32(lo, hi);
        }
        self.pipes.push(Pipe {
            x,
            gap_y,
            scored: false,
        });
    }

    fn check_score(&mut self, out: &mut SoundSet) {
        for p in self.pipes.iter_mut() {
            if !p.scored && BIRD_X - BIRD_HIT_HALF_W > p.x + PIPE_W as f32 {
                p.scored = true;
                self.score += 1;
                out.insert(Sound::Score);
                self.speed = (BASE_SPEED + SPEED_PER_POINT * self.score as f32).min(MAX_SPEED);
            }
        }
    }

    fn check_collisions(&mut self, out: &mut SoundSet) {
        let (bx0, by0, bx1, by1) = self.bird_hitbox();
        // Ceiling.
        if by0 <= 0.0 {
            self.bird_y = BIRD_HIT_HALF_H;
            self.die(out);
            return;
        }
        // Ground.
        if by1 >= PLAY_H as f32 {
            self.bird_y = PLAY_H as f32 - BIRD_HALF_H;
            self.die(out);
            // Set after `die`, which resets the flag for the airborne cases.
            self.landed = true;
            return;
        }
        // Pipes: exact overlap with the rectangles that are drawn. The broad
        // phase must include the cap overhang on *both* sides.
        let cap_reach = PIPE_CAP_OVERHANG as f32;
        for p in &self.pipes {
            if bx1 <= p.x - cap_reach || bx0 >= p.x + (PIPE_W + PIPE_CAP_OVERHANG) as f32 {
                continue;
            }
            for (rx, ry, rw, rh) in p.colliders() {
                if bx0 < rx + rw && bx1 > rx && by0 < ry + rh && by1 > ry {
                    self.die(out);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Framebuffer;

    fn playing(seed: u64) -> Game {
        let mut g = Game::new(seed);
        g.input(Input::KeyDown(Key::Flap));
        g.input(Input::KeyUp(Key::Flap));
        assert_eq!(g.phase(), Phase::Playing);
        g
    }

    /// Advances in realistic frame-sized chunks. A single large `advance` is
    /// clamped (by design), so tests must not use one to skip time.
    fn run_for(g: &mut Game, secs: f32) {
        let n = (secs / FIXED_DT).round() as u32;
        for _ in 0..n {
            g.advance(FIXED_DT);
        }
    }

    fn tap(g: &mut Game) -> SoundSet {
        let mut a = g.input(Input::KeyDown(Key::Flap));
        a.merge(g.input(Input::KeyUp(Key::Flap)));
        a
    }

    #[test]
    fn first_flap_starts_the_run() {
        let mut g = Game::new(1);
        assert_eq!(g.phase(), Phase::Ready);
        assert!(g.pipes().is_empty(), "ready screen must not hold pipes");
        let s = g.input(Input::KeyDown(Key::Flap));
        assert_eq!(g.phase(), Phase::Playing);
        assert!(s.contains(Sound::Flap));
        assert_eq!(g.pipes().len(), 1, "starting spawns the first pipe");
    }

    #[test]
    fn one_physical_press_produces_one_flap() {
        let mut g = playing(2);
        g.advance(0.2);
        let first = g.input(Input::KeyDown(Key::Flap));
        assert!(first.contains(Sound::Flap));
        // Auto-repeat: more key-downs with no key-up must do nothing.
        for _ in 0..10 {
            let repeat = g.input(Input::KeyDown(Key::Flap));
            assert!(repeat.is_empty(), "key repeat leaked through as a flap");
        }
        g.input(Input::KeyUp(Key::Flap));
        assert!(g.input(Input::KeyDown(Key::Flap)).contains(Sound::Flap));
    }

    #[test]
    fn losing_focus_clears_held_keys() {
        let mut g = playing(3);
        g.input(Input::KeyDown(Key::Flap));
        g.set_suspended(true);
        g.set_suspended(false);
        // Without the reset, the key-up that never arrived would leave the key
        // stuck down and swallow this press entirely.
        let s = g.input(Input::KeyDown(Key::Flap));
        assert!(!s.is_empty(), "the fresh press was swallowed by stale state");
        assert_eq!(g.phase(), Phase::Playing);
        g.input(Input::KeyUp(Key::Flap));
        assert!(g.input(Input::KeyDown(Key::Flap)).contains(Sound::Flap));
    }

    #[test]
    fn buttons_never_also_flap() {
        // Mute on the ready screen.
        let mut g = Game::new(4);
        let mute = g.button_rect(Button::Mute);
        let s = g.input(Input::PointerDown {
            x: (mute.x + 2) as f32,
            y: (mute.y + 2) as f32,
        });
        assert!(!s.contains(Sound::Flap));
        assert_eq!(g.phase(), Phase::Ready, "mute must not start the game");
        assert!(g.muted());

        // Pause while playing.
        let mut g = playing(5);
        let pause = g.button_rect(Button::Pause);
        let s = g.input(Input::PointerDown {
            x: (pause.x + pause.w - 1) as f32,
            y: (pause.y + pause.h - 1) as f32,
        });
        assert!(!s.contains(Sound::Flap));
        assert_eq!(g.phase(), Phase::Paused);

        // Resume button: a tap on it resumes without flapping.
        let s = g.input(Input::PointerDown {
            x: (pause.x + 1) as f32,
            y: (pause.y + 1) as f32,
        });
        assert!(!s.contains(Sound::Flap));
        assert_eq!(g.phase(), Phase::Playing);

        // Restart button on the game-over panel.
        g.bird_y = PLAY_H as f32;
        run_for(&mut g, 2.0);
        assert_eq!(g.phase(), Phase::GameOver);
        assert!(g.can_restart());
        let r = g.button_rect(Button::Restart);
        let s = g.input(Input::PointerDown {
            x: (r.x + r.w / 2) as f32,
            y: (r.y + r.h / 2) as f32,
        });
        assert!(!s.contains(Sound::Flap));
        assert_eq!(g.phase(), Phase::Ready);
    }

    #[test]
    fn fatal_tap_cannot_instantly_restart() {
        let mut g = playing(6);
        g.bird_y = PLAY_H as f32;
        g.advance(1.0 / 120.0);
        assert_eq!(g.phase(), Phase::GameOver);
        // A stray tap in the same instant is ignored.
        assert!(g.input(Input::PointerDown { x: 180.0, y: 300.0 }).is_empty());
        assert_eq!(g.phase(), Phase::GameOver);
    }

    #[test]
    fn a_tap_resumes_without_flapping() {
        let mut g = playing(7);
        run_for(&mut g, 0.4);
        let y_before = g.bird_hitbox().1;
        g.input(Input::KeyDown(Key::Pause));
        assert_eq!(g.phase(), Phase::Paused);
        g.input(Input::KeyUp(Key::Pause));

        let s = g.input(Input::PointerDown { x: 180.0, y: 300.0 });
        assert!(!s.contains(Sound::Flap), "the resume tap must be consumed");
        assert_eq!(g.phase(), Phase::Playing);
        g.advance(FIXED_DT);
        // Only gravity acted on the bird, no upward impulse.
        assert!(g.bird_hitbox().1 > y_before);
    }

    #[test]
    fn pause_freezes_the_world() {
        let mut g = playing(8);
        run_for(&mut g, 0.4);
        let snapshot = (g.bird_y, g.score, g.pipes.iter().map(|p| p.x).collect::<Vec<_>>());
        g.input(Input::KeyDown(Key::Pause));
        for _ in 0..600 {
            g.advance(1.0 / 60.0);
        }
        assert_eq!(g.bird_y, snapshot.0);
        assert_eq!(g.score, snapshot.1);
        assert_eq!(
            g.pipes.iter().map(|p| p.x).collect::<Vec<_>>(),
            snapshot.2,
            "pipes moved while paused"
        );
    }

    #[test]
    fn suspension_pauses_and_requires_explicit_resume() {
        let mut g = playing(9);
        run_for(&mut g, 0.2);
        g.set_suspended(true);
        assert_eq!(g.phase(), Phase::Paused);
        assert!(g.paused_by_focus());

        let frozen = g.bird_y;
        // A ten second stall must not advance or fast-forward anything.
        assert!(g.advance(10.0).is_empty());
        assert_eq!(g.bird_y, frozen);

        g.set_suspended(false);
        assert_eq!(g.phase(), Phase::Paused, "must not auto-resume");
        g.advance(0.001);
        assert_eq!(g.bird_y, frozen);

        g.input(Input::KeyDown(Key::Flap)); // first flap resumes, and flaps
        assert_eq!(g.phase(), Phase::Playing);
        assert!(!g.paused_by_focus());
    }

    #[test]
    fn a_huge_frame_delta_cannot_teleport_the_bird() {
        let mut g = playing(10);
        run_for(&mut g, 0.05);
        let before = g.bird_y;
        let s = g.advance(60.0);
        let max_travel = MAX_SUBSTEPS as f32 * FIXED_DT * MAX_FALL_SPEED;
        assert!(
            g.bird_y - before <= max_travel + 1.0,
            "a stalled frame moved the bird {} px",
            g.bird_y - before
        );
        assert!(g.score() <= 1, "a stalled frame scored more than one pipe");
        let _ = s;
    }

    /// Same inputs, same simulated time, four different frame rates.
    #[test]
    fn physics_are_frame_rate_independent() {
        let flaps: Vec<f32> = (0..40).map(|i| i as f32 * 0.4).collect();
        let run = |dt: f32| {
            let mut g = Game::new(0xABCD);
            tap(&mut g);
            let mut t = 0.0f32;
            let mut next = 0usize;
            let mut guard = 0;
            while t < 20.0 && g.phase() == Phase::Playing {
                while next < flaps.len() && flaps[next] <= t + 1e-4 {
                    tap(&mut g);
                    next += 1;
                }
                g.advance(dt);
                t += dt;
                guard += 1;
                assert!(guard < 500_000);
            }
            (g.score(), g.phase(), g.bird_y)
        };
        let base = run(FIXED_DT);
        for k in [2.0f32, 4.0, 8.0] {
            let other = run(FIXED_DT * k);
            assert_eq!(other.0, base.0, "score differs at dt = FIXED_DT * {k}");
            assert_eq!(other.1, base.1, "outcome differs at dt = FIXED_DT * {k}");
            assert!(
                (other.2 - base.2).abs() < 1.0,
                "bird height drifted by {} px at dt = FIXED_DT * {k}",
                (other.2 - base.2).abs()
            );
        }
    }

    #[test]
    fn dt_of_zero_changes_nothing() {
        let mut g = playing(11);
        run_for(&mut g, 0.3);
        let snapshot = (g.bird_y, g.score, g.pipes.iter().map(|p| p.x).collect::<Vec<_>>());
        for _ in 0..100 {
            g.advance(0.0);
        }
        assert_eq!(g.bird_y, snapshot.0);
        assert_eq!(g.pipes.iter().map(|p| p.x).collect::<Vec<_>>(), snapshot.2);
    }

    // -- collision ---------------------------------------------------------

    fn place(g: &mut Game, pipe_x: f32, gap_y: i32, bird_y: f32) {
        g.pipes.clear();
        g.pipes.push(Pipe {
            x: pipe_x,
            gap_y,
            scored: false,
        });
        g.bird_y = bird_y;
        g.bird_vy = 0.0;
        // Freeze the pipes so the test measures collision geometry alone; the
        // real step moves them first, then collides against the drawn position.
        g.speed = 0.0;
    }

    #[test]
    fn pipe_collision_matches_the_drawn_rectangles() {
        let mut g = playing(12);
        let gap_y = 300;
        let gt = gap_y - PIPE_GAP / 2;
        // Bird centre y that leaves its box just inside the gap / just into the
        // upper pipe body. The box spans +-BIRD_HIT_HALF_H around the centre.
        let safe_y = gt as f32 + BIRD_HIT_HALF_H + 0.5;
        let touching_y = gt as f32 + BIRD_HIT_HALF_H - 0.5;

        // Box clears the upper body by half a pixel: no hit.
        place(&mut g, 100.0, gap_y, safe_y);
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::Playing, "false positive inside the gap");

        // Overlapping the upper body by half a pixel: hit.
        place(&mut g, 100.0, gap_y, touching_y);
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::GameOver, "missed a body overlap");

        // The cap overhangs 6px. At x = 112 the cap starts at 106 and the
        // bird's box ends exactly at 106, so it must NOT connect...
        let mut g = playing(13);
        place(&mut g, 112.0, gap_y, touching_y);
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::Playing, "cap bound is not exact");

        // ...but one pixel further left it does (body still does not reach).
        let mut g = playing(14);
        place(&mut g, 110.0, gap_y, touching_y);
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::GameOver, "cap overhang was not collidable");
    }

    #[test]
    fn colliders_are_the_rectangles_the_renderer_fills() {
        let p = Pipe {
            x: 40.0,
            gap_y: 300,
            scored: false,
        };
        let c = p.colliders();
        assert_eq!(c[0], (40.0, 0.0, PIPE_W as f32, p.gap_top() as f32));
        assert_eq!(
            c[1],
            (40.0, p.gap_bottom() as f32, PIPE_W as f32, (PLAY_H - p.gap_bottom()) as f32)
        );
        assert_eq!(
            c[2],
            (
                40.0 - PIPE_CAP_OVERHANG as f32,
                (p.gap_top() - PIPE_CAP_H) as f32,
                (PIPE_W + 2 * PIPE_CAP_OVERHANG) as f32,
                PIPE_CAP_H as f32
            )
        );
        assert_eq!(
            c[3],
            (
                40.0 - PIPE_CAP_OVERHANG as f32,
                p.gap_bottom() as f32,
                (PIPE_W + 2 * PIPE_CAP_OVERHANG) as f32,
                PIPE_CAP_H as f32
            )
        );
    }

    #[test]
    fn ceiling_and_ground_end_the_run() {
        let mut g = playing(15);
        g.bird_y = BIRD_HIT_HALF_H - 1.0;
        g.bird_vy = 0.0;
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::GameOver, "ceiling did not end the run");

        let mut g = playing(16);
        g.bird_y = PLAY_H as f32 - BIRD_HIT_HALF_H + 1.0;
        g.bird_vy = 0.0;
        g.advance(FIXED_DT);
        assert_eq!(g.phase(), Phase::GameOver, "ground did not end the run");
        assert!(g.landed);
    }

    #[test]
    fn scoring_happens_once_per_pipe_pair() {
        let mut g = playing(17);
        g.pipes.clear();
        g.pipes.push(Pipe {
            x: BIRD_X - BIRD_HIT_HALF_W - PIPE_W as f32 - 1.0,
            gap_y: 300,
            scored: false,
        });
        g.bird_y = 300.0;
        g.bird_vy = 0.0;
        g.advance(FIXED_DT);
        assert_eq!(g.score(), 1);
        assert!(g.pipes[0].scored);
        for _ in 0..600 {
            if g.phase() != Phase::Playing {
                break;
            }
            g.bird_y = 300.0;
            g.bird_vy = 0.0;
            g.advance(FIXED_DT);
        }
        assert_eq!(g.score(), 1, "the same pipe scored twice");
    }

    // -- pipes -------------------------------------------------------------

    #[test]
    fn generated_gaps_are_inside_the_playfield_and_reachable() {
        let mut g = Game::new(18);
        let min_center = GAP_EDGE_MARGIN + PIPE_GAP / 2;
        let max_center = PLAY_H - GAP_EDGE_MARGIN - PIPE_GAP / 2;
        let mut prev: Option<i32> = None;
        for i in 0..400 {
            g.spawn_pipe(2000.0 + i as f32);
            let p = *g.pipes.last().unwrap();
            assert!(
                (min_center..=max_center).contains(&p.gap_y),
                "gap centre {} outside [{min_center}, {max_center}]",
                p.gap_y
            );
            assert!(p.gap_top() >= GAP_EDGE_MARGIN);
            assert!(p.gap_bottom() <= PLAY_H - GAP_EDGE_MARGIN);
            if let Some(prev) = prev {
                assert!(
                    (p.gap_y - prev).abs() <= MAX_GAP_DELTA,
                    "gap jumped {} px, more than the reachable limit",
                    (p.gap_y - prev).abs()
                );
            }
            prev = Some(p.gap_y);
        }
        // Randomised: a run of identical gaps would mean the RNG is not wired up.
        let distinct: std::collections::HashSet<i32> =
            g.pipes.iter().map(|p| p.gap_y).collect();
        assert!(distinct.len() > 50, "gap heights are not varied");
    }

    #[test]
    fn pipes_stay_bounded_over_a_long_run() {
        let mut g = Game::new(19);
        let mut max_seen = 0usize;
        for _ in 0..120_000 {
            if g.phase() != Phase::Playing {
                g.input(Input::KeyDown(Key::Flap));
                g.input(Input::KeyUp(Key::Flap));
            }
            // Autopilot: keep the bird near the gap it is approaching so the
            // run lasts long enough to exercise recycling.
            if let Some(p) = g
                .pipes
                .iter()
                .find(|p| p.x + PIPE_W as f32 > BIRD_X - BIRD_HIT_HALF_W)
            {
                let target = p.gap_y as f32;
                if g.bird_y > target + 6.0 && g.bird_vy > -60.0 {
                    g.input(Input::KeyDown(Key::Flap));
                    g.input(Input::KeyUp(Key::Flap));
                }
            }
            g.advance(FIXED_DT);
            max_seen = max_seen.max(g.pipes.len());
            assert!(g.pipes.len() <= MAX_PIPES, "pipe list grew unbounded");
            for p in &g.pipes {
                assert!(p.x.is_finite() && p.gap_y > 0);
            }
        }
        assert!(max_seen >= 2, "never had more than one pipe on screen");
        assert!(g.best() > 0 || g.score() > 0, "autopilot never scored");
    }

    #[test]
    fn restart_resets_gameplay_but_keeps_the_best_score() {
        let mut g = playing(20);
        g.score = 5;
        g.bird_y = PLAY_H as f32;
        run_for(&mut g, 2.0);
        assert_eq!(g.phase(), Phase::GameOver);
        assert!(g.can_restart());

        let s = g.input(Input::KeyDown(Key::Flap));
        assert!(s.contains(Sound::Click));
        assert_eq!(g.phase(), Phase::Ready);
        assert_eq!(g.score(), 0);
        assert_eq!(g.best(), 5);
        assert!(!g.new_best);
        assert!(g.pipes().is_empty());
        assert_eq!(g.bird_y, READY_BIRD_Y);
        assert_eq!(g.speed, BASE_SPEED);
        assert!(g.acc == 0.0);
        assert!(!g.landed);
        assert_eq!(g.panel_t, 0.0);
    }

    #[test]
    fn best_score_is_recorded_once_per_run() {
        let mut g = playing(21);
        g.score = 7;
        g.bird_y = PLAY_H as f32;
        run_for(&mut g, 2.0);
        assert_eq!(g.best(), 7);
        assert!(g.take_settings_dirty());
        assert!(!g.take_settings_dirty(), "dirty flag must clear");

        // A worse run must not lower it.
        tap(&mut g);
        g.score = 2;
        g.bird_y = PLAY_H as f32;
        run_for(&mut g, 2.0);
        assert_eq!(g.best(), 7);
        assert!(!g.new_best);
    }

    // -- rendering ---------------------------------------------------------

    #[test]
    fn every_state_paints_the_whole_playfield() {
        let mut g = Game::new(22);
        let mut fb = Framebuffer::new(LOGICAL_W, LOGICAL_H);
        let mut check = |g: &Game, label: &str| {
            fb.clear(0);
            g.draw(&mut fb);
            let holes = fb.px.iter().filter(|p| **p == 0).count();
            assert_eq!(holes, 0, "{label}: {holes} unpainted pixels");
        };
        check(&g, "ready");
        g.input(Input::KeyDown(Key::Flap));
        g.input(Input::KeyUp(Key::Flap));
        run_for(&mut g, 5.0);
        check(&g, "playing");
        g.input(Input::KeyDown(Key::Pause));
        check(&g, "paused");
        g.input(Input::KeyDown(Key::Pause));
        g.bird_y = PLAY_H as f32;
        run_for(&mut g, 2.0);
        check(&g, "game over");
    }

    #[test]
    fn bird_is_always_drawn_inside_the_playfield() {
        let mut g = Game::new(23);
        for _ in 0..2000 {
            let y = g.bird_draw_y();
            assert!(
                y - BIRD_HALF_H > -2.0 && y + BIRD_HALF_H < PLAY_H as f32 + 2.0,
                "bird drawn at {y}, outside the playfield"
            );
            g.advance(FIXED_DT);
            if g.phase() == Phase::GameOver {
                break;
            }
        }
    }
}
