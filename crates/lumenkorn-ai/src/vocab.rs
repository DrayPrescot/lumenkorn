//! Vokabular für die automatische Verschlagwortung.
//!
//! CLIP versteht Englisch am besten. Jedes Konzept hat deshalb ein deutsches
//! Schlagwort (das im Katalog landet) und englische Formulierungen für das Modell.
//! Konzepte sind in Gruppen sortiert; innerhalb einer Gruppe konkurrieren sie
//! miteinander und mit einer neutralen Formulierung – gewinnt die neutrale,
//! wird aus dieser Gruppe nichts vergeben. Das verhindert, dass jedes Foto
//! irgendein „Wetter“ bekommt.

#[derive(Debug, Clone)]
pub struct Concept {
    pub tag: &'static str,
    pub phrase: &'static str,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: &'static str,
    /// Neutrale Formulierung, die „nichts davon“ vertritt.
    pub neutral: &'static str,
    /// Mindestwahrscheinlichkeit innerhalb der Gruppe.
    pub min_p: f32,
    /// Mehrere Schlagworte aus dieser Gruppe zulassen (z. B. Hund *und* Person).
    pub max_tags: usize,
    pub concepts: Vec<Concept>,
}

/// Vorlagen, die für jedes Konzept gemittelt werden.
pub const TEMPLATES: &[&str] = &["a photo of {}.", "a picture of {}.", "{}"];

macro_rules! c {
    ($tag:expr, $phrase:expr) => {
        Concept {
            tag: $tag,
            phrase: $phrase,
        }
    };
}

pub fn default_groups() -> Vec<Group> {
    vec![
        Group {
            name: "Ort",
            neutral: "something",
            min_p: 0.30,
            max_tags: 2,
            concepts: vec![
                c!("Strand", "a sandy beach"),
                c!("Meer", "the sea"),
                c!("See", "a lake"),
                c!("Fluss", "a river"),
                c!("Wasserfall", "a waterfall"),
                c!("Berge", "mountains"),
                c!("Wald", "a forest"),
                c!("Wiese", "a green meadow"),
                c!("Feld", "farm fields"),
                c!("Wüste", "a desert"),
                c!("Stadt", "a city street"),
                c!("Skyline", "a city skyline"),
                c!("Dorf", "a village"),
                c!("Park", "a park"),
                c!("Garten", "a garden"),
                c!("Innenraum", "an indoor room"),
                c!("Küche", "a kitchen"),
                c!("Restaurant", "a restaurant"),
                c!("Bar", "a bar or pub"),
                c!("Kirche", "a church"),
                c!("Bühne", "a concert stage"),
                c!("Studio", "a photo studio with a plain backdrop"),
                c!("Hafen", "a harbor with boats"),
                c!("Brücke", "a bridge"),
                c!("Straße", "a road"),
                c!("Bahnhof", "a train station"),
                c!("Weltraum", "outer space or earth seen from orbit"),
            ],
        },
        Group {
            name: "Motiv",
            neutral: "an object",
            min_p: 0.22,
            max_tags: 3,
            concepts: vec![
                c!("Person", "a person"),
                c!("Personengruppe", "a group of people"),
                c!("Porträt", "a portrait of a face"),
                c!("Kind", "a child"),
                c!("Baby", "a baby"),
                c!("Hochzeit", "a wedding couple"),
                c!("Hund", "a dog"),
                c!("Katze", "a cat"),
                c!("Pferd", "a horse"),
                c!("Vogel", "a bird"),
                c!("Wildtier", "a wild animal"),
                c!("Insekt", "an insect"),
                c!("Blume", "a flower"),
                c!("Baum", "a tree"),
                c!("Essen", "a plate of food"),
                c!("Getränk", "a drink in a glass"),
                c!("Bier", "a glass of beer"),
                c!("Auto", "a car"),
                c!("Fahrrad", "a bicycle"),
                c!("Motorrad", "a motorcycle"),
                c!("Zug", "a train"),
                c!("Flugzeug", "an airplane"),
                c!("Boot", "a boat"),
                c!("Gebäude", "a building"),
                c!("Denkmal", "a monument or statue"),
                c!("Schuhe", "shoes"),
                c!("Produkt", "a product shot of an object"),
                c!("Dokument", "a document with text"),
                c!("Polarlicht", "the aurora borealis"),
                c!("Feuerwerk", "fireworks"),
            ],
        },
        Group {
            name: "Licht",
            neutral: "a normal daytime photo",
            min_p: 0.40,
            max_tags: 1,
            concepts: vec![
                c!("Sonnenuntergang", "a sunset or sunrise"),
                c!("Nacht", "a photo taken at night"),
                c!("Blaue Stunde", "the blue hour at dusk"),
                c!("Gegenlicht", "a backlit scene against the sun"),
                c!("Kunstlicht", "a scene lit by colored artificial light"),
            ],
        },
        Group {
            name: "Wetter",
            neutral: "clear weather",
            min_p: 0.40,
            max_tags: 1,
            concepts: vec![
                c!("Schnee", "snow"),
                c!("Regen", "rain"),
                c!("Nebel", "fog or mist"),
                c!("Gewitter", "dramatic storm clouds"),
                c!("Herbst", "autumn foliage"),
                c!("Frühling", "spring blossoms"),
            ],
        },
        Group {
            name: "Stil",
            neutral: "an ordinary color photo",
            min_p: 0.45,
            max_tags: 1,
            concepts: vec![
                c!("Schwarzweiß", "a black and white photo"),
                c!("Makro", "a macro close-up photo"),
                c!("Luftaufnahme", "an aerial drone photo"),
                c!(
                    "Langzeitbelichtung",
                    "a long exposure photo with motion blur"
                ),
                c!("Unscharf", "a blurry out of focus photo"),
                c!("Screenshot", "a screenshot of a computer screen"),
            ],
        },
    ]
}

/// Deutsche Wörter für die semantische Suche ins Englische übertragen – CLIP
/// versteht deutsche Anfragen kaum. Unbekannte Wörter bleiben unverändert.
pub fn translate_query(query: &str) -> String {
    let groups = default_groups();
    let mut words: Vec<String> = Vec::new();
    for raw in query.split_whitespace() {
        let w = raw
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        let hit = groups
            .iter()
            .flat_map(|g| &g.concepts)
            .find(|c| c.tag.to_lowercase() == w)
            .map(|c| strip_article(c.phrase).to_string())
            .or_else(|| {
                EXTRA
                    .iter()
                    .find(|(de, _)| *de == w)
                    .map(|(_, en)| en.to_string())
            });
        words.push(hit.unwrap_or_else(|| raw.to_string()));
    }
    words.join(" ")
}

fn strip_article(s: &str) -> &str {
    for a in ["a ", "an ", "the "] {
        if let Some(rest) = s.strip_prefix(a) {
            return rest;
        }
    }
    s
}

const EXTRA: &[(&str, &str)] = &[
    ("am", "at the"),
    ("im", "in the"),
    ("an", "at"),
    ("auf", "on"),
    ("mit", "with"),
    ("und", "and"),
    ("bei", "at"),
    ("unter", "under"),
    ("vor", "in front of"),
    ("ohne", "without"),
    ("der", "the"),
    ("die", "the"),
    ("das", "the"),
    ("ein", "a"),
    ("eine", "a"),
    ("einem", "a"),
    ("einer", "a"),
    ("rot", "red"),
    ("grün", "green"),
    ("blau", "blue"),
    ("gelb", "yellow"),
    ("weiß", "white"),
    ("schwarz", "black"),
    ("orange", "orange"),
    ("lila", "purple"),
    ("rosa", "pink"),
    ("braun", "brown"),
    ("frau", "woman"),
    ("mann", "man"),
    ("mädchen", "girl"),
    ("junge", "boy"),
    ("leute", "people"),
    ("menschen", "people"),
    ("haus", "house"),
    ("himmel", "sky"),
    ("wolken", "clouds"),
    ("sonne", "sun"),
    ("mond", "moon"),
    ("sterne", "stars"),
    ("wasser", "water"),
    ("eis", "ice"),
    ("feuer", "fire"),
    ("lachen", "laughing"),
    ("lachend", "laughing"),
    ("tanzen", "dancing"),
    ("laufen", "running"),
    ("hunde", "dogs"),
    ("katzen", "cats"),
    ("autos", "cars"),
    ("füße", "feet"),
    ("beine", "legs"),
    ("schatten", "shadows"),
    ("licht", "light"),
    ("dunkel", "dark"),
    ("hell", "bright"),
    ("winter", "winter"),
    ("sommer", "summer"),
    ("urlaub", "vacation"),
    ("essen", "food"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_known_words() {
        assert_eq!(translate_query("Hund am Strand"), "dog at the sandy beach");
        assert_eq!(translate_query("Eiffelturm"), "Eiffelturm");
    }

    #[test]
    fn tags_are_unique() {
        let mut tags: Vec<&str> = default_groups()
            .iter()
            .flat_map(|g| g.concepts.iter().map(|c| c.tag))
            .collect();
        let n = tags.len();
        tags.sort();
        tags.dedup();
        assert_eq!(tags.len(), n);
    }
}
