//! Tierarten als Schlagworte, von grob nach fein: Säugetier → Affe →
//! Schimpanse, Vogel → Meise → Blaumeise.
//!
//! Dasselbe CLIP-Modell wie für die übrigen Schlagworte vergleicht das Bild
//! mit allen Arten und Gruppen auf einmal (Zero-Shot), zusammen mit typischen
//! Nicht-Tier-Motiven (Person, Landschaft, Spielzeug, Statue …) – gewinnen
//! die, gibt es keine Tier-Schlagworte. Die Wahrscheinlichkeit einer Gruppe
//! ist die Summe ihrer Arten; abgestiegen wird zur jeweils
//! wahrscheinlichsten Untergruppe, solange sie sicher genug ist: lieber
//! „Vogel“ als eine geratene Art. Jede erreichte Ebene wird ein Schlagwort.

use crate::clip::{Clip, dot, mean_normalized};
use crate::error::Result;
use crate::tagger::TagSuggestion;
use crate::vocab::TEMPLATES;

/// Name der Gruppe in den Vorschlägen
const GROUP: &str = "Tier";
/// CLIP-Temperatur (wie in `tagger`)
const LOGIT_SCALE: f32 = 100.0;
/// Ist ein Tier zu sehen? Anteil der Tierklassen (und allgemeiner
/// Tier-Beschreibungen) gegen die Nicht-Tier-Motive
const GATE_P: f32 = 0.5;
/// Mindestwahrscheinlichkeit einer Ebene (Anteil an „Tier“, Gruppe samt
/// Unterarten) je Tiefe: Klasse, Gruppe, Art. Darunter endet der Abstieg.
const LEVEL_P: [f32; 3] = [0.6, 0.45, 0.4];
/// Ist die wahrscheinlichste Art (bzw. Gruppe ohne Unterarten) mindestens so
/// sicher, gilt ihr ganzer Pfad – auch wenn sich der Rest auf viele
/// Geschwister verteilt (bei Vögeln die Regel).
const LEAF_P: f32 = 0.33;

/// Ein Knoten der Taxonomie: Schlagwort deutsch/englisch, englische
/// Beschreibung für CLIP und Unterarten.
pub struct Taxon {
    pub tag: &'static str,
    pub en: &'static str,
    pub phrase: &'static str,
    pub children: &'static [Taxon],
}

impl Taxon {
    /// Schlagwort in der Sprache der Oberfläche
    pub fn name(&self) -> &'static str {
        lumenkorn_develop::i18n::tr_s(self.tag, self.en)
    }
}

const fn t(
    tag: &'static str,
    en: &'static str,
    phrase: &'static str,
    children: &'static [Taxon],
) -> Taxon {
    Taxon {
        tag,
        en,
        phrase,
        children,
    }
}

/// Art ohne Unterteilung
const fn s(tag: &'static str, en: &'static str, phrase: &'static str) -> Taxon {
    t(tag, en, phrase, &[])
}

/// Motive, die kein Tier sind – gewinnen sie, gibt es keine Tier-Schlagworte
const NOT_ANIMAL: &[&str] = &[
    "a person",
    "a group of people",
    "a landscape",
    "a building",
    "a street in a city",
    "food on a plate",
    "a car or vehicle",
    "a plant or flower",
    "a tree",
    "leaves and branches",
    "an everyday object",
    "a stuffed toy animal",
    "a statue or sculpture",
    "a drawing or painting",
    "text or a document",
];

/// Allgemeine Tier-Beschreibungen für die Vorprüfung (neben den Klassen)
const ANIMAL_HINTS: &[&str] = &[
    "an animal",
    "a pet dog",
    "a pet cat",
    "a horse",
    "a wild animal",
];

/// Die Taxonomie: oberste Ebene sind die Tierklassen.
pub static TAXONOMY: &[Taxon] = &[
    t("Säugetier", "Mammal", "a mammal", MAMMALS),
    t("Vogel", "Bird", "a bird", BIRDS),
    t("Reptil", "Reptile", "a reptile", REPTILES),
    t(
        "Amphibie",
        "Amphibian",
        "an amphibian such as a frog or salamander",
        AMPHIBIANS,
    ),
    t("Fisch", "Fish", "a fish", FISH),
    t("Insekt", "Insect", "an insect", INSECTS),
    t("Spinnentier", "Arachnid", "a spider or scorpion", ARACHNIDS),
    t(
        "Meerestier",
        "Sea creature",
        "a sea creature such as a jellyfish, starfish or crab",
        SEA,
    ),
    s("Schnecke", "Snail", "a snail"),
];

static MAMMALS: &[Taxon] = &[
    t(
        "Affe",
        "Monkey",
        "a monkey or ape",
        &[
            s("Schimpanse", "Chimpanzee", "a chimpanzee"),
            s("Gorilla", "Gorilla", "a gorilla"),
            s("Orang-Utan", "Orangutan", "an orangutan"),
            s("Pavian", "Baboon", "a baboon"),
            s("Makak", "Macaque", "a macaque monkey"),
            s("Kapuzineraffe", "Capuchin monkey", "a capuchin monkey"),
            s("Totenkopfaffe", "Squirrel monkey", "a squirrel monkey"),
            s("Gibbon", "Gibbon", "a gibbon"),
            s("Lemur", "Lemur", "a ring-tailed lemur"),
        ],
    ),
    t(
        "Katze",
        "Cat",
        "a cat or big cat",
        &[
            s("Hauskatze", "Domestic cat", "a pet house cat"),
            s("Löwe", "Lion", "a lion"),
            s("Tiger", "Tiger", "a tiger"),
            s("Leopard", "Leopard", "a leopard"),
            s("Gepard", "Cheetah", "a cheetah"),
            s("Jaguar", "Jaguar", "a jaguar"),
            s("Luchs", "Lynx", "a lynx"),
            s("Schneeleopard", "Snow leopard", "a snow leopard"),
        ],
    ),
    t(
        "Hundeartige",
        "Canine",
        "a dog, wolf or fox",
        &[
            s("Hund", "Dog", "a pet dog"),
            s("Wolf", "Wolf", "a wolf"),
            s("Fuchs", "Fox", "a red fox"),
            s("Polarfuchs", "Arctic fox", "an arctic fox"),
        ],
    ),
    t(
        "Bär",
        "Bear",
        "a bear",
        &[
            s("Braunbär", "Brown bear", "a brown bear"),
            s("Eisbär", "Polar bear", "a polar bear"),
            s("Schwarzbär", "Black bear", "an american black bear"),
            s("Großer Panda", "Giant panda", "a giant panda"),
        ],
    ),
    t(
        "Pferd",
        "Horse",
        "a horse, donkey or zebra",
        &[
            s("Hauspferd", "Domestic horse", "a horse"),
            s("Pony", "Pony", "a pony"),
            s("Esel", "Donkey", "a donkey"),
            s("Zebra", "Zebra", "a zebra"),
        ],
    ),
    t(
        "Rind",
        "Cattle",
        "a cow or bovine animal",
        &[
            s("Kuh", "Cow", "a cow"),
            s("Wasserbüffel", "Water buffalo", "a water buffalo"),
            s("Bison", "Bison", "a bison"),
            s("Yak", "Yak", "a yak"),
            s(
                "Hochlandrind",
                "Highland cattle",
                "a highland cow with long hair",
            ),
        ],
    ),
    t(
        "Hirsch",
        "Deer",
        "a deer",
        &[
            s("Rothirsch", "Red deer", "a red deer stag"),
            s("Reh", "Roe deer", "a roe deer"),
            s("Damhirsch", "Fallow deer", "a fallow deer"),
            s("Elch", "Moose", "a moose"),
            s("Rentier", "Reindeer", "a reindeer"),
        ],
    ),
    t(
        "Ziege und Schaf",
        "Goat and sheep",
        "a goat or sheep",
        &[
            s("Ziege", "Goat", "a goat"),
            s("Schaf", "Sheep", "a sheep"),
            s("Steinbock", "Ibex", "an alpine ibex"),
            s("Gämse", "Chamois", "a chamois"),
            s("Mufflon", "Mouflon", "a mouflon"),
        ],
    ),
    t(
        "Schwein",
        "Pig",
        "a pig or boar",
        &[
            s("Hausschwein", "Domestic pig", "a domestic pig"),
            s("Wildschwein", "Wild boar", "a wild boar"),
            s("Warzenschwein", "Warthog", "a warthog"),
        ],
    ),
    t(
        "Antilope",
        "Antelope",
        "an antelope",
        &[
            s("Gazelle", "Gazelle", "a gazelle"),
            s("Gnu", "Wildebeest", "a wildebeest"),
            s("Impala", "Impala", "an impala"),
            s("Springbock", "Springbok", "a springbok"),
            s("Oryx", "Oryx", "an oryx"),
        ],
    ),
    t(
        "Kamel",
        "Camel",
        "a camel or llama",
        &[
            s("Dromedar", "Dromedary", "a dromedary camel with one hump"),
            s(
                "Trampeltier",
                "Bactrian camel",
                "a bactrian camel with two humps",
            ),
            s("Lama", "Llama", "a llama"),
            s("Alpaka", "Alpaca", "an alpaca"),
        ],
    ),
    t(
        "Elefant",
        "Elephant",
        "an elephant",
        &[
            s(
                "Afrikanischer Elefant",
                "African elephant",
                "an african elephant",
            ),
            s("Asiatischer Elefant", "Asian elephant", "an asian elephant"),
        ],
    ),
    s("Giraffe", "Giraffe", "a giraffe"),
    s("Nashorn", "Rhinoceros", "a rhinoceros"),
    s("Flusspferd", "Hippopotamus", "a hippopotamus"),
    t(
        "Nagetier",
        "Rodent",
        "a rodent",
        &[
            s("Eichhörnchen", "Squirrel", "a red squirrel"),
            s("Streifenhörnchen", "Chipmunk", "a chipmunk"),
            s("Maus", "Mouse", "a mouse"),
            s("Ratte", "Rat", "a rat"),
            s("Hamster", "Hamster", "a hamster"),
            s("Meerschweinchen", "Guinea pig", "a guinea pig"),
            s("Biber", "Beaver", "a beaver"),
            s("Murmeltier", "Marmot", "a marmot"),
            s("Capybara", "Capybara", "a capybara"),
        ],
    ),
    t(
        "Hase und Kaninchen",
        "Hare and rabbit",
        "a hare or rabbit",
        &[
            s("Feldhase", "Hare", "a brown hare"),
            s("Kaninchen", "Rabbit", "a rabbit"),
        ],
    ),
    t(
        "Marder",
        "Mustelid",
        "a weasel, otter or badger",
        &[
            s("Fischotter", "Otter", "a river otter"),
            s("Dachs", "Badger", "a badger"),
            s("Steinmarder", "Marten", "a marten"),
            s("Frettchen", "Ferret", "a ferret"),
            s("Hermelin", "Stoat", "a stoat"),
        ],
    ),
    t(
        "Meeressäuger",
        "Marine mammal",
        "a marine mammal",
        &[
            s("Delfin", "Dolphin", "a dolphin"),
            s("Wal", "Whale", "a whale"),
            s("Robbe", "Seal", "a seal"),
            s("Seelöwe", "Sea lion", "a sea lion"),
            s("Walross", "Walrus", "a walrus"),
            s("Seeotter", "Sea otter", "a sea otter"),
        ],
    ),
    t(
        "Beuteltier",
        "Marsupial",
        "a marsupial",
        &[
            s("Känguru", "Kangaroo", "a kangaroo"),
            s("Koala", "Koala", "a koala"),
            s("Wombat", "Wombat", "a wombat"),
        ],
    ),
    s("Igel", "Hedgehog", "a hedgehog"),
    s("Fledermaus", "Bat", "a bat"),
    s("Waschbär", "Raccoon", "a raccoon"),
    s("Erdmännchen", "Meerkat", "a meerkat"),
    s("Faultier", "Sloth", "a sloth"),
    s("Roter Panda", "Red panda", "a red panda"),
];

static BIRDS: &[Taxon] = &[
    t(
        "Meise",
        "Tit",
        "a tit bird",
        &[
            s("Kohlmeise", "Great tit", "a great tit"),
            s("Blaumeise", "Blue tit", "a eurasian blue tit"),
            s("Tannenmeise", "Coal tit", "a coal tit"),
            s("Schwanzmeise", "Long-tailed tit", "a long-tailed tit"),
            s("Sumpfmeise", "Marsh tit", "a marsh tit"),
        ],
    ),
    t(
        "Fink",
        "Finch",
        "a finch",
        &[
            s("Buchfink", "Chaffinch", "a common chaffinch"),
            s("Grünfink", "Greenfinch", "a european greenfinch"),
            s("Stieglitz", "Goldfinch", "a european goldfinch"),
            s("Gimpel", "Bullfinch", "a eurasian bullfinch"),
            s("Kernbeißer", "Hawfinch", "a hawfinch"),
        ],
    ),
    t(
        "Drossel",
        "Thrush",
        "a thrush",
        &[
            s("Amsel", "Blackbird", "a common blackbird"),
            s("Singdrossel", "Song thrush", "a song thrush"),
            s("Wacholderdrossel", "Fieldfare", "a fieldfare"),
        ],
    ),
    t(
        "Sperling",
        "Sparrow",
        "a sparrow",
        &[
            s("Haussperling", "House sparrow", "a house sparrow"),
            s("Feldsperling", "Tree sparrow", "a eurasian tree sparrow"),
        ],
    ),
    t(
        "Rabenvogel",
        "Corvid",
        "a crow, raven, magpie or jay",
        &[
            s("Rabenkrähe", "Carrion crow", "a carrion crow"),
            s("Kolkrabe", "Raven", "a common raven"),
            s("Elster", "Magpie", "a eurasian magpie"),
            s("Eichelhäher", "Jay", "a eurasian jay"),
            s("Dohle", "Jackdaw", "a western jackdaw"),
        ],
    ),
    t(
        "Greifvogel",
        "Bird of prey",
        "a bird of prey",
        &[
            s("Mäusebussard", "Buzzard", "a common buzzard"),
            s("Turmfalke", "Kestrel", "a common kestrel"),
            s("Wanderfalke", "Peregrine falcon", "a peregrine falcon"),
            s("Rotmilan", "Red kite", "a red kite"),
            s("Habicht", "Goshawk", "a northern goshawk"),
            s("Sperber", "Sparrowhawk", "a eurasian sparrowhawk"),
            s("Steinadler", "Golden eagle", "a golden eagle"),
            s("Seeadler", "White-tailed eagle", "a white-tailed eagle"),
            s("Weißkopfseeadler", "Bald eagle", "a bald eagle"),
            s("Fischadler", "Osprey", "an osprey"),
            s("Geier", "Vulture", "a vulture"),
        ],
    ),
    t(
        "Eule",
        "Owl",
        "an owl",
        &[
            s("Waldkauz", "Tawny owl", "a tawny owl"),
            s("Schleiereule", "Barn owl", "a barn owl"),
            s("Uhu", "Eagle-owl", "a eurasian eagle-owl"),
            s("Waldohreule", "Long-eared owl", "a long-eared owl"),
            s("Schneeeule", "Snowy owl", "a snowy owl"),
            s("Steinkauz", "Little owl", "a little owl"),
        ],
    ),
    t(
        "Specht",
        "Woodpecker",
        "a woodpecker",
        &[
            s(
                "Buntspecht",
                "Great spotted woodpecker",
                "a great spotted woodpecker",
            ),
            s(
                "Grünspecht",
                "Green woodpecker",
                "a european green woodpecker",
            ),
            s("Schwarzspecht", "Black woodpecker", "a black woodpecker"),
        ],
    ),
    t(
        "Entenvogel",
        "Waterfowl",
        "a duck, goose or swan",
        &[
            s("Stockente", "Mallard", "a mallard duck"),
            s("Höckerschwan", "Mute swan", "a mute swan"),
            s("Graugans", "Greylag goose", "a greylag goose"),
            s("Kanadagans", "Canada goose", "a canada goose"),
            s("Nilgans", "Egyptian goose", "an egyptian goose"),
            s("Reiherente", "Tufted duck", "a tufted duck"),
            s("Mandarinente", "Mandarin duck", "a mandarin duck"),
            s("Hausgans", "Domestic goose", "a white domestic goose"),
        ],
    ),
    t(
        "Möwe",
        "Gull",
        "a seagull",
        &[
            s("Lachmöwe", "Black-headed gull", "a black-headed gull"),
            s("Silbermöwe", "Herring gull", "a herring gull"),
        ],
    ),
    t(
        "Schreitvogel",
        "Wading bird",
        "a heron, stork or crane",
        &[
            s("Graureiher", "Grey heron", "a grey heron"),
            s("Silberreiher", "Great egret", "a great egret"),
            s("Weißstorch", "White stork", "a white stork"),
            s("Kranich", "Crane", "a common crane"),
            s("Flamingo", "Flamingo", "a flamingo"),
        ],
    ),
    t(
        "Taube",
        "Pigeon",
        "a pigeon or dove",
        &[
            s("Straßentaube", "Feral pigeon", "a feral city pigeon"),
            s("Ringeltaube", "Wood pigeon", "a common wood pigeon"),
            s("Türkentaube", "Collared dove", "a eurasian collared dove"),
        ],
    ),
    t(
        "Papagei",
        "Parrot",
        "a parrot",
        &[
            s("Wellensittich", "Budgerigar", "a budgerigar"),
            s("Ara", "Macaw", "a macaw"),
            s("Kakadu", "Cockatoo", "a cockatoo"),
            s("Graupapagei", "Grey parrot", "an african grey parrot"),
            s(
                "Halsbandsittich",
                "Ring-necked parakeet",
                "a rose-ringed parakeet",
            ),
        ],
    ),
    t(
        "Pinguin",
        "Penguin",
        "a penguin",
        &[
            s("Kaiserpinguin", "Emperor penguin", "an emperor penguin"),
            s("Königspinguin", "King penguin", "a king penguin"),
            s("Eselspinguin", "Gentoo penguin", "a gentoo penguin"),
            s("Humboldtpinguin", "Humboldt penguin", "a humboldt penguin"),
        ],
    ),
    t(
        "Hühnervogel",
        "Gamebird",
        "a chicken, pheasant or peacock",
        &[
            s("Haushuhn", "Chicken", "a chicken"),
            s("Fasan", "Pheasant", "a common pheasant"),
            s("Pfau", "Peacock", "a peacock"),
            s("Truthahn", "Turkey", "a turkey"),
        ],
    ),
    s("Rotkehlchen", "Robin", "a european robin"),
    s("Zaunkönig", "Wren", "a eurasian wren"),
    s("Bachstelze", "Wagtail", "a white wagtail"),
    s("Star", "Starling", "a common starling"),
    s("Eisvogel", "Kingfisher", "a common kingfisher"),
    s("Schwalbe", "Swallow", "a barn swallow"),
    s("Kolibri", "Hummingbird", "a hummingbird"),
    s("Strauß", "Ostrich", "an ostrich"),
    s("Pelikan", "Pelican", "a pelican"),
    s("Kormoran", "Cormorant", "a cormorant"),
    s("Tukan", "Toucan", "a toucan"),
    s(
        "Haubentaucher",
        "Great crested grebe",
        "a great crested grebe",
    ),
    s("Blässhuhn", "Coot", "a eurasian coot"),
    s("Wiedehopf", "Hoopoe", "a hoopoe"),
    s("Kuckuck", "Cuckoo", "a common cuckoo"),
    s("Papageitaucher", "Puffin", "an atlantic puffin"),
];

static REPTILES: &[Taxon] = &[
    t(
        "Schildkröte",
        "Turtle",
        "a turtle or tortoise",
        &[
            s("Landschildkröte", "Tortoise", "a tortoise"),
            s("Meeresschildkröte", "Sea turtle", "a sea turtle"),
            s("Wasserschildkröte", "Pond turtle", "a pond turtle"),
        ],
    ),
    t(
        "Echse",
        "Lizard",
        "a lizard",
        &[
            s("Zauneidechse", "Sand lizard", "a small sand lizard"),
            s("Leguan", "Iguana", "an iguana"),
            s("Gecko", "Gecko", "a gecko"),
            s("Chamäleon", "Chameleon", "a chameleon"),
            s("Waran", "Monitor lizard", "a monitor lizard"),
            s("Bartagame", "Bearded dragon", "a bearded dragon lizard"),
        ],
    ),
    t(
        "Schlange",
        "Snake",
        "a snake",
        &[
            s("Ringelnatter", "Grass snake", "a grass snake"),
            s("Kreuzotter", "Adder", "a common european adder"),
            s("Python", "Python", "a python"),
            s("Kobra", "Cobra", "a cobra"),
            s("Klapperschlange", "Rattlesnake", "a rattlesnake"),
        ],
    ),
    t(
        "Panzerechse",
        "Crocodilian",
        "a crocodile or alligator",
        &[
            s("Krokodil", "Crocodile", "a crocodile"),
            s("Alligator", "Alligator", "an alligator"),
        ],
    ),
];

static AMPHIBIANS: &[Taxon] = &[
    t(
        "Frosch",
        "Frog",
        "a frog",
        &[
            s("Laubfrosch", "Tree frog", "a green tree frog"),
            s("Grasfrosch", "Common frog", "a common frog"),
            s(
                "Pfeilgiftfrosch",
                "Poison dart frog",
                "a colorful poison dart frog",
            ),
        ],
    ),
    s("Kröte", "Toad", "a toad"),
    s("Salamander", "Salamander", "a fire salamander"),
    s("Molch", "Newt", "a newt"),
];

static FISH: &[Taxon] = &[
    s("Hai", "Shark", "a shark"),
    s("Rochen", "Ray", "a stingray"),
    s("Clownfisch", "Clownfish", "a clownfish"),
    s("Goldfisch", "Goldfish", "a goldfish"),
    s("Koi", "Koi", "a koi carp"),
    s("Lachs", "Salmon", "a salmon"),
    s("Forelle", "Trout", "a trout"),
    s("Seepferdchen", "Seahorse", "a seahorse"),
    s("Kugelfisch", "Pufferfish", "a pufferfish"),
];

static INSECTS: &[Taxon] = &[
    t(
        "Schmetterling",
        "Butterfly",
        "a butterfly or moth",
        &[
            s(
                "Tagpfauenauge",
                "Peacock butterfly",
                "a european peacock butterfly",
            ),
            s("Admiral", "Red admiral", "a red admiral butterfly"),
            s("Zitronenfalter", "Brimstone", "a brimstone butterfly"),
            s(
                "Kleiner Fuchs",
                "Small tortoiseshell",
                "a small tortoiseshell butterfly",
            ),
            s("Schwalbenschwanz", "Swallowtail", "a swallowtail butterfly"),
            s("Kohlweißling", "Cabbage white", "a cabbage white butterfly"),
            s("Monarchfalter", "Monarch butterfly", "a monarch butterfly"),
            s("Nachtfalter", "Moth", "a moth"),
        ],
    ),
    t(
        "Biene",
        "Bee",
        "a bee",
        &[
            s("Honigbiene", "Honey bee", "a honey bee"),
            s("Hummel", "Bumblebee", "a bumblebee"),
        ],
    ),
    t(
        "Käfer",
        "Beetle",
        "a beetle",
        &[
            s("Marienkäfer", "Ladybird", "a ladybug"),
            s("Hirschkäfer", "Stag beetle", "a stag beetle"),
            s("Maikäfer", "Cockchafer", "a cockchafer"),
            s("Rosenkäfer", "Rose chafer", "a green rose chafer beetle"),
        ],
    ),
    s("Libelle", "Dragonfly", "a dragonfly"),
    s("Wespe", "Wasp", "a wasp"),
    s("Heuschrecke", "Grasshopper", "a grasshopper"),
    s("Ameise", "Ant", "an ant"),
    s("Fliege", "Fly", "a fly"),
    s("Gottesanbeterin", "Praying mantis", "a praying mantis"),
    s("Raupe", "Caterpillar", "a caterpillar"),
];

static ARACHNIDS: &[Taxon] = &[
    s("Spinne", "Spider", "a spider"),
    s("Skorpion", "Scorpion", "a scorpion"),
];

static SEA: &[Taxon] = &[
    s("Qualle", "Jellyfish", "a jellyfish"),
    s("Seestern", "Starfish", "a starfish"),
    s("Krabbe", "Crab", "a crab"),
    s("Hummer", "Lobster", "a lobster"),
    s("Krake", "Octopus", "an octopus"),
    s("Seeigel", "Sea urchin", "a sea urchin"),
];

/// Ein Knoten in der flachen Liste (Tiefensuche: Eltern vor ihren Kindern)
struct Node {
    taxon: &'static Taxon,
    /// Index des Elternknotens, `None` für die Tierklassen
    parent: Option<usize>,
    depth: usize,
    embedding: Vec<f32>,
}

pub struct AnimalTagger {
    nodes: Vec<Node>,
    /// Nicht-Tier-Motive und allgemeine Tier-Beschreibungen für die Vorprüfung
    others: Vec<Vec<f32>>,
    hints: Vec<Vec<f32>>,
}

/// Alle Knoten der Taxonomie in fester Reihenfolge (Tiefensuche)
fn walk(list: &'static [Taxon], out: &mut Vec<&'static Taxon>) {
    for t in list {
        out.push(t);
        walk(t.children, out);
    }
}

/// Wie [`walk`], mit Elternindex, Tiefe und der Tierklasse (für die
/// Beschreibung „…, a type of bird“)
fn walk_nodes(
    list: &'static [Taxon],
    parent: Option<usize>,
    depth: usize,
    class: Option<&'static str>,
    out: &mut Vec<(&'static Taxon, Option<usize>, usize, Option<&'static str>)>,
) {
    for t in list {
        let i = out.len();
        out.push((t, parent, depth, class));
        let kind = class.or_else(|| {
            CLASS_KIND
                .iter()
                .find(|(c, _)| *c == t.tag)
                .map(|(_, k)| *k)
        });
        walk_nodes(t.children, Some(i), depth + 1, kind, out);
    }
}

/// Wofür eine Tierklasse in den Beschreibungen ihrer Arten steht
const CLASS_KIND: &[(&str, &str)] = &[
    ("Säugetier", "mammal"),
    ("Vogel", "bird"),
    ("Reptil", "reptile"),
    ("Amphibie", "amphibian"),
    ("Fisch", "fish"),
    ("Insekt", "insect"),
    ("Spinnentier", "arachnid"),
    ("Meerestier", "sea animal"),
];

impl AnimalTagger {
    /// Text-Embeddings aller Beschreibungen berechnen (einmal, in Stapeln).
    pub fn new(clip: &Clip) -> Result<Self> {
        let mut flat = Vec::new();
        walk_nodes(TAXONOMY, None, 0, None, &mut flat);
        // Unterhalb der Klasse mit Zusatz „…, a type of bird“: hilft CLIP, eine
        // Art der richtigen Klasse zuzuordnen (Rotkehlchen ist ein Vogel)
        let phrases: Vec<String> = NOT_ANIMAL
            .iter()
            .chain(ANIMAL_HINTS)
            .map(|p| p.to_string())
            .chain(flat.iter().map(|(t, _, depth, kind)| match kind {
                Some(k) if *depth > 0 => format!("{}, a type of {k}", t.phrase),
                _ => t.phrase.to_string(),
            }))
            .collect();
        let per = TEMPLATES.len();
        let texts: Vec<String> = phrases
            .iter()
            .flat_map(|p| TEMPLATES.iter().map(move |t| t.replace("{}", p)))
            .collect();
        let mut rows = Vec::with_capacity(texts.len());
        for batch in texts.chunks(64) {
            let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
            rows.extend(clip.embed_texts(&refs)?);
        }
        let mut emb = rows.chunks(per).map(mean_normalized);
        let others = emb.by_ref().take(NOT_ANIMAL.len()).collect();
        let hints = emb.by_ref().take(ANIMAL_HINTS.len()).collect();
        let nodes = flat
            .into_iter()
            .zip(emb)
            .map(|((taxon, parent, depth, _), embedding)| Node {
                taxon,
                parent,
                depth,
                embedding,
            })
            .collect();
        Ok(Self {
            nodes,
            others,
            hints,
        })
    }

    /// Anteil „Tier“: Klassen und allgemeine Tier-Beschreibungen gegen die
    /// Nicht-Tier-Motive. Getrennt von der Artbestimmung – dort stünden
    /// 300 Tierbeschreibungen gegen eine Handvoll anderer, und schon die Menge
    /// gäbe jedem Foto ein Tier.
    fn animal_share(&self, image: &[f32]) -> f32 {
        let logit = |e: &Vec<f32>| dot(e, image) * LOGIT_SCALE;
        let classes = self
            .nodes
            .iter()
            .filter(|n| n.depth == 0)
            .map(|n| &n.embedding);
        let animal: Vec<f32> = classes.chain(&self.hints).map(logit).collect();
        let mut all = animal.clone();
        all.extend(self.others.iter().map(logit));
        softmax(&all)[..animal.len()].iter().sum()
    }

    /// Wahrscheinlichkeit je Knoten unter allen Tierknoten: eigene und mit
    /// Unterarten aufsummierte (Kinder stehen hinter den Eltern)
    fn probabilities(&self, image: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let own = softmax(
            &self
                .nodes
                .iter()
                .map(|n| dot(&n.embedding, image) * LOGIT_SCALE)
                .collect::<Vec<_>>(),
        );
        let mut agg = own.clone();
        for i in (0..self.nodes.len()).rev() {
            if let Some(parent) = self.nodes[i].parent {
                agg[parent] += agg[i];
            }
        }
        (own, agg)
    }

    /// Schlagworte für ein Bild-Embedding: die erreichten Ebenen, grob zuerst.
    pub fn tag(&self, image: &[f32]) -> Vec<TagSuggestion> {
        let animal = self.animal_share(image);
        if animal < GATE_P {
            return Vec::new();
        }
        let (own, agg) = self.probabilities(image);
        // Pfad zur wahrscheinlichsten Art (Klasse zuerst)
        let best = (0..self.nodes.len())
            .max_by(|a, b| own[*a].total_cmp(&own[*b]))
            .expect("Taxonomie nicht leer");
        let mut path = vec![best];
        while let Some(parent) = self.nodes[*path.last().expect("Pfad")].parent {
            path.push(parent);
        }
        path.reverse();
        // Sichere Art: ganzer Pfad. Sonst so weit, wie die Ebenen sicher sind.
        let sure = own[best] >= LEAF_P;
        let mut out = Vec::new();
        for &i in &path {
            let node = &self.nodes[i];
            if !sure && agg[i] < LEVEL_P[node.depth.min(LEVEL_P.len() - 1)] {
                break;
            }
            out.push(TagSuggestion {
                tag: node.taxon.name().to_string(),
                group: GROUP.to_string(),
                confidence: (agg[i] * animal).min(1.0),
            });
        }
        out
    }
}

fn softmax(x: &[f32]) -> Vec<f32> {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = x.iter().map(|v| (v - max).exp()).collect();
    let s: f32 = e.iter().sum();
    e.into_iter().map(|v| v / s).collect()
}

/// Englische Beschreibung zu einem Tiernamen (ein Wort, klein geschrieben) –
/// für die Suche nach Bildinhalt („blaumeise“ → „a eurasian blue tit“).
/// Deutsche Namen nur bei deutscher Oberfläche (wie im übrigen Vokabular).
pub fn phrase_for(word: &str, english_ui: bool) -> Option<&'static str> {
    let mut all = Vec::new();
    walk(TAXONOMY, &mut all);
    all.into_iter()
        .find(|t| t.en.to_lowercase() == word || (!english_ui && t.tag.to_lowercase() == word))
        .map(|t| t.phrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_is_consistent() {
        let mut all = Vec::new();
        walk(TAXONOMY, &mut all);
        assert!(all.len() > 250, "{}", all.len());
        // Höchstens drei Ebenen (so viele Schwellen gibt es)
        fn depth(l: &[Taxon]) -> usize {
            l.iter().map(|t| 1 + depth(t.children)).max().unwrap_or(0)
        }
        assert!(depth(TAXONOMY) <= LEVEL_P.len());
        // Kein Schlagwort doppelt (außer bewusst gleichnamige Ebenen)
        let mut names: Vec<&str> = all.iter().map(|t| t.tag).collect();
        names.sort_unstable();
        let dups: Vec<_> = names
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(dups.is_empty(), "{dups:?}");
        let mut en: Vec<&str> = all.iter().map(|t| t.en).collect();
        en.sort_unstable();
        let dups: Vec<_> = en
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(dups.is_empty(), "{dups:?}");
        assert!(all.iter().all(|t| !t.phrase.is_empty() && !t.en.is_empty()));
    }

    #[test]
    fn search_words_find_animals() {
        assert_eq!(phrase_for("blaumeise", false), Some("a eurasian blue tit"));
        assert_eq!(phrase_for("blaumeise", true), None);
        assert_eq!(phrase_for("chimpanzee", true), Some("a chimpanzee"));
        assert_eq!(phrase_for("auto", false), None);
    }
}
