//! Vokabular für die automatische Verschlagwortung.
//!
//! CLIP versteht Englisch am besten. Jedes Konzept hat deshalb ein Schlagwort
//! (deutsch bzw. englisch je nach Sprache der Oberfläche – das landet im Katalog)
//! und eine englische Formulierung für das Modell.
//! Konzepte sind in Gruppen sortiert; innerhalb einer Gruppe konkurrieren sie
//! miteinander und mit einer neutralen Formulierung – gewinnt die neutrale,
//! wird aus dieser Gruppe nichts vergeben. Das verhindert, dass jedes Foto
//! irgendein „Wetter“ bekommt.

#[derive(Debug, Clone)]
pub struct Concept {
    /// Deutsches Schlagwort
    pub tag: &'static str,
    /// Englisches Schlagwort
    pub en: &'static str,
    pub phrase: &'static str,
}

impl Concept {
    /// Schlagwort in der Sprache der Oberfläche
    pub fn name(&self) -> &'static str {
        lumenkorn_develop::i18n::tr_s(self.tag, self.en)
    }
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
    ($tag:expr, $en:expr, $phrase:expr) => {
        Concept {
            tag: $tag,
            en: $en,
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
                c!("Strand", "Beach", "a sandy beach"),
                c!("Meer", "Sea", "the sea"),
                c!("See", "Lake", "a lake"),
                c!("Fluss", "River", "a river"),
                c!("Wasserfall", "Waterfall", "a waterfall"),
                c!("Berge", "Mountains", "mountains"),
                c!("Wald", "Forest", "a forest"),
                c!("Wiese", "Meadow", "a green meadow"),
                c!("Feld", "Fields", "farm fields"),
                c!("Wüste", "Desert", "a desert"),
                c!("Stadt", "City", "a city street"),
                c!("Skyline", "Skyline", "a city skyline"),
                c!("Dorf", "Village", "a village"),
                c!("Park", "Park", "a park"),
                c!("Garten", "Garden", "a garden"),
                c!("Innenraum", "Indoor", "an indoor room"),
                c!("Küche", "Kitchen", "a kitchen"),
                c!("Restaurant", "Restaurant", "a restaurant"),
                c!("Bar", "Bar", "a bar or pub"),
                c!("Kirche", "Church", "a church"),
                c!("Bühne", "Stage", "a concert stage"),
                c!("Studio", "Studio", "a photo studio with a plain backdrop"),
                c!("Hafen", "Harbor", "a harbor with boats"),
                c!("Brücke", "Bridge", "a bridge"),
                c!("Straße", "Road", "a road"),
                c!("Bahnhof", "Train station", "a train station"),
                c!("Weltraum", "Space", "outer space or earth seen from orbit"),
            ],
        },
        Group {
            name: "Motiv",
            neutral: "an object",
            min_p: 0.22,
            max_tags: 3,
            concepts: vec![
                c!("Person", "Person", "a person"),
                c!("Personengruppe", "Group of people", "a group of people"),
                c!("Porträt", "Portrait", "a portrait of a face"),
                c!("Kind", "Child", "a child"),
                c!("Baby", "Baby", "a baby"),
                c!("Hochzeit", "Wedding", "a wedding couple"),
                c!("Hund", "Dog", "a dog"),
                c!("Katze", "Cat", "a cat"),
                c!("Pferd", "Horse", "a horse"),
                c!("Vogel", "Bird", "a bird"),
                c!("Wildtier", "Wildlife", "a wild animal"),
                c!("Insekt", "Insect", "an insect"),
                c!("Blume", "Flower", "a flower"),
                c!("Baum", "Tree", "a tree"),
                c!("Essen", "Food", "a plate of food"),
                c!("Getränk", "Drink", "a drink in a glass"),
                c!("Bier", "Beer", "a glass of beer"),
                c!("Auto", "Car", "a car"),
                c!("Fahrrad", "Bicycle", "a bicycle"),
                c!("Motorrad", "Motorcycle", "a motorcycle"),
                c!("Zug", "Train", "a train"),
                c!("Flugzeug", "Airplane", "an airplane"),
                c!("Boot", "Boat", "a boat"),
                c!("Gebäude", "Building", "a building"),
                c!("Denkmal", "Monument", "a monument or statue"),
                c!("Schuhe", "Shoes", "shoes"),
                c!("Produkt", "Product", "a product shot of an object"),
                c!("Dokument", "Document", "a document with text"),
                c!("Polarlicht", "Aurora", "the aurora borealis"),
                c!("Feuerwerk", "Fireworks", "fireworks"),
            ],
        },
        Group {
            name: "Licht",
            neutral: "a normal daytime photo",
            min_p: 0.40,
            max_tags: 1,
            concepts: vec![
                c!("Sonnenuntergang", "Sunset", "a sunset or sunrise"),
                c!("Nacht", "Night", "a photo taken at night"),
                c!("Blaue Stunde", "Blue hour", "the blue hour at dusk"),
                c!("Gegenlicht", "Backlight", "a backlit scene against the sun"),
                c!(
                    "Kunstlicht",
                    "Artificial light",
                    "a scene lit by colored artificial light"
                ),
            ],
        },
        Group {
            name: "Wetter",
            neutral: "clear weather",
            min_p: 0.40,
            max_tags: 1,
            concepts: vec![
                c!("Schnee", "Snow", "snow"),
                c!("Regen", "Rain", "rain"),
                c!("Nebel", "Fog", "fog or mist"),
                c!("Gewitter", "Storm", "dramatic storm clouds"),
                c!("Herbst", "Autumn", "autumn foliage"),
                c!("Frühling", "Spring", "spring blossoms"),
            ],
        },
        Group {
            name: "Stil",
            neutral: "an ordinary color photo",
            min_p: 0.45,
            max_tags: 1,
            concepts: vec![
                c!("Schwarzweiß", "Black and white", "a black and white photo"),
                c!("Makro", "Macro", "a macro close-up photo"),
                c!("Luftaufnahme", "Aerial", "an aerial drone photo"),
                c!(
                    "Langzeitbelichtung",
                    "Long exposure",
                    "a long exposure photo with motion blur"
                ),
                c!("Unscharf", "Blurry", "a blurry out of focus photo"),
                c!(
                    "Screenshot",
                    "Screenshot",
                    "a screenshot of a computer screen"
                ),
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
            .find(|c| c.tag.to_lowercase() == w || c.en.to_lowercase() == w)
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
        for name in [|c: &Concept| c.tag, |c: &Concept| c.en] {
            let mut tags: Vec<&str> = default_groups()
                .iter()
                .flat_map(|g| g.concepts.iter().map(name))
                .collect();
            let n = tags.len();
            tags.sort();
            tags.dedup();
            assert_eq!(tags.len(), n);
        }
    }

    #[test]
    fn english_keywords_translate_too() {
        assert_eq!(
            translate_query("dog on the Beach"),
            "dog on the sandy beach"
        );
    }
}
