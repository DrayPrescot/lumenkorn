// Vorher/Nachher-Regler: der Schieber setzt die Breite des oberen Bildes.
for (const box of document.querySelectorAll(".compare")) {
  const input = box.querySelector("input");
  const set = () => box.style.setProperty("--pos", input.value + "%");
  input.addEventListener("input", set);
  set();
}

// Sprachwahl merken, damit die Startseite nicht wieder umleitet
for (const a of document.querySelectorAll("a[hreflang]")) {
  a.addEventListener("click", () => {
    try {
      localStorage.setItem("lang", a.hreflang);
    } catch {}
  });
}
