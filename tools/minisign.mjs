// Prüft minisign-Signaturen so wie der Updater in der App. Braucht nur den
// öffentlichen Schlüssel, also weder Schlüsseldatei noch Passwort.

import { createHash, createPublicKey, verify } from "node:crypto";

const decode = (text) => Buffer.from(text.trim(), "base64");

/** Öffentlicher Schlüssel, wie er in tauri.conf.json steht */
export function parsePublicKey(pubkey) {
  const raw = decode(decode(pubkey).toString("utf8").split(/\r?\n/)[1] ?? "");
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== "Ed") throw new Error("Öffentlicher Schlüssel unlesbar");
  const x = raw.subarray(10).toString("base64url");
  return { id: raw.subarray(2, 10), key: createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x }, format: "jwk" }) };
}

/**
 * Inhalt einer .sig-Datei gegen die Daten prüfen. Wirft bei jeder Abweichung und
 * liefert sonst den mitsignierten Kommentar als Objekt (file, version, timestamp).
 */
export function verifySignature(publicKey, data, signature) {
  const [, encoded, commentLine, global] = decode(signature).toString("utf8").split(/\r?\n/);
  const sig = decode(encoded ?? "");
  // „ED“: signiert ist der BLAKE2b-Hash der Datei. Tauri erzeugt nur diese Form.
  if (sig.length !== 74 || sig.subarray(0, 2).toString() !== "ED") throw new Error("Signatur unlesbar");
  if (!sig.subarray(2, 10).equals(publicKey.id)) throw new Error("mit einem anderen Schlüssel signiert");

  const bytes = sig.subarray(10);
  const hash = createHash("blake2b512").update(data).digest();
  if (!verify(null, hash, publicKey.key, bytes)) throw new Error("Signatur passt nicht zur Datei");

  const PREFIX = "trusted comment: ";
  if (!commentLine?.startsWith(PREFIX)) throw new Error("Signatur unlesbar");
  const comment = commentLine.slice(PREFIX.length);
  const signed = Buffer.concat([bytes, Buffer.from(comment, "utf8")]);
  if (!verify(null, signed, publicKey.key, decode(global ?? ""))) throw new Error("Kommentar der Signatur wurde verändert");

  const fields = comment.split("\t").map((part) => {
    const colon = part.indexOf(":");
    return [part.slice(0, colon), part.slice(colon + 1)];
  });
  return Object.fromEntries(fields);
}
