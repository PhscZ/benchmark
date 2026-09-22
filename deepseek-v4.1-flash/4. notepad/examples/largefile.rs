//! Measures the editor's core operations against the generated fixtures.
//!
//! This exercises the library directly (no GUI), so it isolates the piece table
//! and the search engine. Run with:
//!
//! ```sh
//! cargo run --release --example largefile
//! ```
//!
//! Every number printed is a wall-clock measurement of a real operation on a real
//! fixture, which is the evidence behind the large-file claims in `README.md`.

use notepad::fileio;
use notepad::motion;
use notepad::progress::Progress;
use notepad::search::{self, Query};
use std::path::Path;
use std::time::Instant;

fn human(n: usize) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f < KIB * KIB {
        format!("{:.1} KiB", f / KIB)
    } else if f < KIB * KIB * KIB {
        format!("{:.1} MiB", f / (KIB * KIB))
    } else {
        format!("{:.2} GiB", f / (KIB * KIB * KIB))
    }
}

fn ms(d: std::time::Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let cases: &[(&str, &str)] = &[
        ("million_lines.txt", "1,000,000 short lines"),
        ("large_90mb.txt", "90 MiB mixed content"),
    ];

    for (name, label) in cases {
        let path = root.join(name);
        if !path.exists() {
            println!("skipping {name}: not found. Run `python tools/make_fixtures.py` first.");
            continue;
        }
        println!("\n=== {name} ({label}) ===");

        // ---- load --------------------------------------------------------
        let progress = Progress::new();
        let t = Instant::now();
        let loaded = match fileio::load(&path, &progress) {
            Ok(l) => l,
            Err(e) => {
                println!("load failed: {e}");
                continue;
            }
        };
        let load_time = t.elapsed();
        let size = loaded.size as usize;
        let mut buffer = loaded.buffer;
        println!(
            "load + UTF-8 validate + index : {:>10}  ({}/s)",
            ms(load_time),
            human((size as f64 / load_time.as_secs_f64()) as usize)
        );
        println!("  size        : {}", human(size));
        println!("  lines       : {}", buffer.line_count());
        println!("  pieces      : {}", buffer.piece_count());
        println!("  resident    : {}", human(buffer.memory_bytes()));

        // ---- line index queries -----------------------------------------
        let lines = buffer.line_count();
        let t = Instant::now();
        let mut checksum = 0usize;
        for i in 0..2000 {
            let line = (i * 7919) % lines;
            checksum = checksum.wrapping_add(buffer.line_start(line));
        }
        let index_time = t.elapsed();
        println!(
            "2000 random line_start()     : {:>10}  ({:.1} us/query, checksum {checksum})",
            ms(index_time),
            index_time.as_secs_f64() * 1e6 / 2000.0
        );

        // ---- caret movement near the end --------------------------------
        let end = buffer.len();
        let t = Instant::now();
        let mut pos = end;
        for _ in 0..10_000 {
            pos = buffer.prev_char(pos);
        }
        let nav_time = t.elapsed();
        println!(
            "10,000 prev_char from the end: {:>10}  ({:.1} us/step)",
            ms(nav_time),
            nav_time.as_secs_f64() * 1e6 / 10_000.0
        );

        // ---- typing -----------------------------------------------------
        let t = Instant::now();
        for i in 0..1000 {
            buffer
                .insert(end, format!("{i}").as_bytes())
                .expect("insert");
        }
        let type_time = t.elapsed();
        println!(
            "1000 keystrokes at the end   : {:>10}  ({:.1} us/keystroke)",
            ms(type_time),
            type_time.as_secs_f64() * 1e6 / 1000.0
        );
        println!("  pieces after typing: {}", buffer.piece_count());

        // ---- undo of that whole run -------------------------------------
        let t = Instant::now();
        for _ in 0..1000 {
            buffer.remove(end, buffer.len());
        }
        println!("1000 deletes back to size    : {:>10}", ms(t.elapsed()));

        // ---- word navigation --------------------------------------------
        let t = Instant::now();
        let mut p = 0usize;
        for _ in 0..1000 {
            p = motion::next_word_boundary(&buffer, p);
            if p >= buffer.len() {
                p = 0;
            }
        }
        let word_time = t.elapsed();
        println!(
            "1000 Ctrl+Right steps        : {:>10}  ({:.1} us/step)",
            ms(word_time),
            word_time.as_secs_f64() * 1e6 / 1000.0
        );

        // ---- snapshot cost ----------------------------------------------
        let t = Instant::now();
        let snap = buffer.snapshot();
        let snap_time = t.elapsed();
        println!(
            "snapshot (for save/search)   : {:>10}  ({} pieces copied by pointer)",
            ms(snap_time),
            snap.pieces().len()
        );

        // ---- search ------------------------------------------------------
        for needle in [
            "SEARCHABLE_TOKEN",
            "NEEDLE_MARKER",
            "Ünïcödé_Töken",
            "zzz-not-present",
        ] {
            let Some(q) = Query::new(needle, true) else {
                continue;
            };
            let t = Instant::now();
            let found = search::find_all(&snap, &q);
            let elapsed = t.elapsed();
            println!(
                "search {needle:<18}: {:>10}  ({} matches, {}/s)",
                ms(elapsed),
                found.len(),
                human((size as f64 / elapsed.as_secs_f64()) as usize)
            );
        }

        // ---- save --------------------------------------------------------
        let out = std::env::temp_dir().join(format!("notepad-bench-{name}"));
        let progress = Progress::new();
        let t = Instant::now();
        let result = fileio::save_atomic(&out, &progress, |w| {
            fileio::write_snapshot(w, &snap, false, &progress)
        });
        match result {
            Ok(bytes) => println!(
                "atomic save                  : {:>10}  ({}/s, {} written)",
                ms(t.elapsed()),
                human((bytes as f64 / t.elapsed().as_secs_f64()) as usize),
                human(bytes as usize)
            ),
            Err(e) => println!("save failed: {e}"),
        }
        let _ = std::fs::remove_file(&out);
    }

    println!("\nDone.");
}
