// Nur auf der englischen Startseite: Besucher mit deutscher Browsersprache
// einmal auf /de/ schicken, solange sie keine Sprache gewählt haben.
try {
  const chosen = localStorage.getItem("lang");
  const lang = (navigator.languages && navigator.languages[0]) || navigator.language || "";
  if (!chosen && lang.toLowerCase().startsWith("de")) location.replace("/de/");
} catch {}
