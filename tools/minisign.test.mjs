import { describe, expect, it } from "vitest";
import { parsePublicKey, verifySignature } from "./minisign.mjs";

// Wegwerfschlüssel, nur für diesen Test erzeugt und nicht aufgehoben:
// `tauri signer sign --app-version 9.9.9 probe.bin`
const PUBLIC =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDBGMDYxRTdGQkY5OUYyRTEKUldUaDhwbS9meDRHRHo5MW1YWnZhbVFMaFJnWVRiamxmSUtDOWQvSXROeE1lVlA4UmRQb0pmSkYK";
const OTHER_PUBLIC =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDFGOUU2REFFQUY0Q0Q3MkEKUldRcTEweXZybTJlSDAvdDVXelRqT1VkYzJ3WDR3eGtPVlI3eTVJTUZUdFlHcWZPZUdiRW1DaXYK";
const SIGNATURE =
  "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUaDhwbS9meDRHRC9ydEwxN2dwVEloUE91TXlSRWxNMHRwOTd2RlZZazVvVzZmcEpSQmJ6SUEyZDZ3WWwyMXlmTnFLZVo3UVk5eE9UcVRsek9SVUR0Nk5PKzlZT2xkR0FnPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzODI2MTA3CWZpbGU6cHJvYmUuYmluCXZlcnNpb246OS45LjkKSkVtSlo3cFc5azdkN0hNM1hZOHRabngreklpR2hHcmtLQkhPc0VrWDZ3ajN1TldzclpwdFhEK1U2ZlFJSkduRXJLNlRtNWZlYmlRdWZnSXFIVVR2QUE9PQo=";
const DATA = Buffer.from("Lumenkorn");

const key = parsePublicKey(PUBLIC);

describe("Signatur eines Installers", () => {
  it("liefert den mitsignierten Kommentar", () => {
    const signed = verifySignature(key, DATA, SIGNATURE);
    expect(signed.version).toBe("9.9.9");
    expect(signed.file).toBe("probe.bin");
  });

  it("lehnt veränderte Daten ab", () => {
    expect(() => verifySignature(key, Buffer.from("Lumenkorn!"), SIGNATURE)).toThrow("passt nicht zur Datei");
  });

  it("lehnt einen anderen Schlüssel ab", () => {
    expect(() => verifySignature(parsePublicKey(OTHER_PUBLIC), DATA, SIGNATURE)).toThrow("anderen Schlüssel");
  });

  // Sonst ließe sich ein altes Release mit neuer Versionsnummer ausliefern
  it("lehnt eine umgeschriebene Version ab", () => {
    const text = Buffer.from(SIGNATURE, "base64").toString("utf8").replace("version:9.9.9", "version:9.9.10");
    expect(() => verifySignature(key, DATA, Buffer.from(text).toString("base64"))).toThrow("Kommentar");
  });

  it("lehnt Unlesbares ab", () => {
    expect(() => verifySignature(key, DATA, "")).toThrow("unlesbar");
    expect(() => verifySignature(key, DATA, "kaputt")).toThrow("unlesbar");
    expect(() => parsePublicKey("kaputt")).toThrow("unlesbar");
  });
});
