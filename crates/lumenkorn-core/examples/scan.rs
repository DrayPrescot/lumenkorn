//! Kommandozeilen-Test des Kerns ohne Oberfläche:
//!
//!     cargo run -p lumenkorn-core --example scan -- <Ordner> [<Katalog.db>]
//!
//! Scannt den Ordner, erzeugt Thumbnails und gibt eine Übersicht aus.

use lumenkorn_core::scanner::scan_root;
use lumenkorn_core::thumbs::{ThumbCache, generate_pending};
use lumenkorn_core::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("Aufruf: scan <Ordner> [<Katalog.db>]"));
    let db = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("lumenkorn-test/katalog.db"));

    let cat = Mutex::new(Catalog::open(&db)?);
    let root = {
        let c = cat.lock().unwrap();
        let norm = paths::normalize_dir(&dir);
        match c.roots()?.into_iter().find(|r| r.path == norm) {
            Some(r) => r,
            None => c.add_root(&dir)?,
        }
    };

    let t = Instant::now();
    let report = scan_root(&cat, root.id, &|p| {
        eprint!("\r{:?} {}/{}      ", p.phase, p.done, p.total)
    })?;
    eprintln!();
    println!("Scan in {:.2?}: {report:?}", t.elapsed());

    let cache = ThumbCache::new(db.parent().unwrap().join("cache"));
    let t = Instant::now();
    let n = generate_pending(&cat, &cache, &|_, _| {})?;
    println!("{n} Thumbnails in {:.2?}", t.elapsed());

    let c = cat.lock().unwrap();
    print_tree(&c.folder_tree(root.id)?, 0);
    for img in c.query_images(&ImageQuery {
        limit: Some(10),
        ..ImageQuery::new(Scope::All)
    })? {
        println!(
            "  {:>6}  {:<40} {:?}",
            img.id, img.file_name, img.captured_at
        );
    }
    Ok(())
}

fn print_tree(n: &FolderNode, depth: usize) {
    println!("{}{} ({}/{})", "  ".repeat(depth), n.name, n.count, n.total);
    for c in &n.children {
        print_tree(c, depth + 1);
    }
}
