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
pub mod geo;
pub mod geotime;
pub mod housekeeping;
pub mod import;
pub mod maskstore;
pub mod metadata;
pub mod model;
pub mod paths;
pub mod people;
pub mod rename;
pub mod scanner;
pub mod stats;
pub mod thumbs;
pub mod timeshift;
pub mod watcher;

pub use catalog::Catalog;
pub use error::{Error, Result};
pub use model::*;

use std::sync::{Mutex, MutexGuard};

/// Sperren ohne Abbruch bei einer „vergifteten“ Sperre. Stürzt ein Auftrag ab,
/// während er den Katalog hält, rollt SQLite die angefangene Transaktion
/// zurück – der Katalog bleibt brauchbar. Mit `lock().unwrap()` wäre er bis
/// zum Neustart für jeden weiteren Befehl gesperrt.
pub trait LockExt<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}
