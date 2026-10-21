// Einstellungen der Oberfläche, die nur dieses Gerät betreffen: Standardordner
// für Export und Fotoalbum. (Das Importziel speichert das Backend in
// import.json, die KI-Einstellungen in ki/einstellungen.json.)

const KEY = "lumenkorn.prefs";

interface Stored {
  exportDir?: string | null;
  albumDir?: string | null;
}

function read(): Stored {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Stored;
  } catch {
    return {};
  }
}

class Prefs {
  /** Exportziel; null: Unterordner „Export“ neben dem jeweiligen Original */
  exportDir = $state<string | null>(read().exportDir ?? null);
  /** Speicherort für Fotoalben; null: wie der Speichern-Dialog zuletzt */
  albumDir = $state<string | null>(read().albumDir ?? null);

  save() {
    try {
      localStorage.setItem(KEY, JSON.stringify({ exportDir: this.exportDir, albumDir: this.albumDir }));
    } catch {
      // Speicher gesperrt: gilt dann nur für diese Sitzung
    }
  }
}

export const prefs = new Prefs();
