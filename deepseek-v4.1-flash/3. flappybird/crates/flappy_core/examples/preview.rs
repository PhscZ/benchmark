//! Throwaway visual check: dumps representative frames as BMPs.
use flappy_core::{Framebuffer, Game, Input, Key, Phase, LOGICAL_H, LOGICAL_W};

fn bmp(fb: &Framebuffer) -> Vec<u8> {
    let w = fb.w as u32;
    let h = fb.h as u32;
    let row = (w * 3).div_ceil(4) * 4;
    let data = row * h;
    let mut out = Vec::with_capacity(54 + data as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let mut written = 0;
        for x in 0..w {
            let p = fb.px[(y * w + x) as usize];
            out.push(p as u8);
            out.push((p >> 8) as u8);
            out.push((p >> 16) as u8);
            written += 3;
        }
        while written < row {
            out.push(0);
            written += 1;
        }
    }
    out
}

fn save(fb: &Framebuffer, name: &str) {
    std::fs::write(name, bmp(fb)).unwrap_or_else(|e| panic!("could not write {name}: {e}"));
    println!("wrote {name}");
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    // Create the destination so `-- <dir>` works on a fresh checkout.
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("could not create {out}: {e}");
        std::process::exit(1);
    }
    let mut fb = Framebuffer::new(LOGICAL_W, LOGICAL_H);

    let mut g = Game::new(0xF1A9);
    g.draw(&mut fb);
    save(&fb, &format!("{out}/01_ready.bmp"));

    // Playing, autopiloted so a few pipes are on screen with a real score.
    g.input(Input::KeyDown(Key::Flap));
    g.input(Input::KeyUp(Key::Flap));
    for _ in 0..2000 {
        if g.phase() != Phase::Playing {
            break;
        }
        if let Some(p) = g.pipes().iter().find(|p| p.x + 62.0 > 86.0) {
            let target = p.gap_y as f32;
            let top = g.bird_hitbox().1;
            if top > target - 7.0 - 6.0 && top < target + 40.0 {
                g.input(Input::KeyDown(Key::Flap));
                g.input(Input::KeyUp(Key::Flap));
            }
        }
        g.advance(1.0 / 120.0);
        if g.score() >= 3 {
            break;
        }
    }
    g.draw(&mut fb);
    save(&fb, &format!("{out}/02_playing.bmp"));

    g.input(Input::KeyDown(Key::Pause));
    g.input(Input::KeyUp(Key::Pause));
    g.draw(&mut fb);
    save(&fb, &format!("{out}/03_paused.bmp"));
    g.input(Input::KeyDown(Key::Pause));
    g.input(Input::KeyUp(Key::Pause));
    assert_eq!(g.phase(), Phase::Playing, "resume failed");

    // Fly straight into the ground to reach the game-over screen.
    for _ in 0..4000 {
        if g.phase() != Phase::Playing {
            break;
        }
        g.advance(1.0 / 120.0);
    }
    for _ in 0..400 {
        g.advance(1.0 / 120.0);
    }
    g.draw(&mut fb);
    save(&fb, &format!("{out}/04_gameover.bmp"));
    println!("phase={:?} score={}", g.phase(), g.score());
}
