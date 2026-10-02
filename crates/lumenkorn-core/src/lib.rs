//! Lumenkorn-Kern.
//!
//! Dieses Crate kennt weder Tauri noch eine Oberfläche. Es verwaltet den
//! Katalog (SQLite), scannt Quellordner direkt auf der Platte, bildet
//! Projekte aus beliebig vielen Ordnern und erzeugt Vorschaubilder.
//! Dadurch lässt es sich auf Desktop und Android identisch verwenden und
//! ohne GUI testen.

pub mod catalog;
pub mod error;
pub mod formats;
pub mod housekeeping;
pub mod maskstore;
pub mod metadata;
pub mod model;
pub mod paths;
pub mod people;
pub mod scanner;
pub mod thumbs;
pub mod watcher;

pub use catalog::Catalog;
pub use error::{Error, Result};
pub use model::*;
