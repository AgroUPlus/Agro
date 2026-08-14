use rand::seq::SliceRandom;

const WORDLIST: &[&str] = &[
    "amber", "anchor", "anthem", "apex", "apolo", "archer", "arctic", "armor", "arrow", "atlas",
    "aurora", "axiom", "azure", "beacon", "blaze", "bolt", "breeze", "bridge", "bronze", "canyon",
    "carbon", "cedar", "cipher", "cliff", "cloud", "clover", "cobalt", "comet", "compass", "coral",
    "cosmos", "crater", "crest", "crystal", "delta", "drift", "dune", "eagle", "echo", "eclipse",
    "ember", "falcon", "feather", "fjord", "flame", "flare", "flint", "forest", "fossil", "frost",
    "galaxy", "glacier", "granite", "grove", "harbor", "haven", "hawk", "helix", "horizon", "hydra",
    "hyper", "impact", "indigo", "island", "jaguar", "jungle", "jupiter", "keystone", "lagoon", "lantern",
    "laser", "legacy", "legend", "lemur", "lightning", "lotus", "lumen", "lunar", "magnet", "mantle",
    "marble", "matrix", "meadow", "meteor", "mirage", "monolith", "nebula", "neon", "nexus", "nova",
    "oasis", "obsidian", "ocean", "octave", "omega", "onyx", "optics", "orbit", "orca", "orion",
    "osprey", "ozone", "pacific", "paladin", "panther", "paradox", "peak", "pelican", "phantom", "phoenix",
    "photon", "pioneer", "plasma", "polar", "prism", "pulsar", "pyramid", "quantum", "quartz", "quasar",
    "radar", "radiant", "raptor", "raven", "reef", "relay", "resonance", "ridge", "river", "rover",
    "ruby", "saber", "safari", "sapphire", "saturn", "scale", "shadow", "shield", "sierra", "signal",
    "silicon", "silver", "siren", "solaris", "sonar", "spark", "spectrum", "sphere", "spiral", "summit",
    "syntax", "talon", "tempest", "terminal", "terra", "timber", "titan", "topaz", "torrent", "tracer",
    "tracker", "transit", "tundra", "twilight", "umbra", "valiant", "valley", "vapor", "vector", "velocity",
    "velvet", "venture", "vertex", "vessel", "vortex", "voyage", "wave", "whisper", "wildfire", "zenith",
    "zephyr", "zero", "zodiac"
];

/// Generate a human-readable, secure passphrase consisting of 4 distinct words.
/// e.g. "cobalt-falcon-orbit-zenith"
pub fn generate_passphrase() -> String {
    let mut rng = rand::thread_rng();
    let words: Vec<&str> = WORDLIST.choose_multiple(&mut rng, 4).cloned().collect();
    words.join("-")
}

const ADJECTIVES: &[&str] = &[
    "Cosmic", "Groovy", "Hyper", "Electric", "Snarky", "Velvet",
    "Funky", "Galactic", "Chill", "Breezy", "Mighty", "Zenith",
    "Neon", "Solar", "Quantum", "Turbo", "Radiant", "Shadow",
    "Glitchy", "Slick", "Wobbly", "Mellow", "Zippy", "Astral",
    "Caffeinated", "Boogie", "Sleepy", "Bouncy", "Dapper", "Spunky"
];

const ANIMALS: &[&str] = &[
    "Capybara", "Otter", "Badger", "Possum", "Wombat", "Quokka",
    "Penguin", "Falcon", "Gecko", "Panda", "Fox", "Lemur",
    "Jaguar", "Raven", "Orca", "Chameleon", "Koala", "Lynx",
    "Pangolin", "Axolotl", "Chinchilla", "Ferret", "Platypus", "Meerkat",
    "Narwhal", "Hedgehog", "Sloth", "Armadillo", "Alpaca", "Walrus"
];

/// Generate a fresh random silly petname for a device upon registration.
/// e.g. "Caffeinated Capybara", "Boogie Wombat", "Snarky Axolotl"
pub fn generate_random_petname() -> String {
    let mut rng = rand::thread_rng();
    let adj = ADJECTIVES.choose(&mut rng).unwrap_or(&"Chill");
    let animal = ANIMALS.choose(&mut rng).unwrap_or(&"Capybara");
    format!("{} {}", adj, animal)
}

/// Backward-compatible petname generator.
pub fn generate_petname(seed: &str) -> String {
    if seed.is_empty() {
        generate_random_petname()
    } else {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        let hash = hasher.finish() as usize;
        let adj = ADJECTIVES[hash % ADJECTIVES.len()];
        let animal = ANIMALS[(hash / ADJECTIVES.len()) % ANIMALS.len()];
        format!("{} {}", adj, animal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_passphrase() {
        let pass = generate_passphrase();
        let parts: Vec<&str> = pass.split('-').collect();
        assert_eq!(parts.len(), 4);
        for part in parts {
            assert!(WORDLIST.contains(&part));
        }
    }

    #[test]
    fn test_generate_random_petname() {
        let name = generate_random_petname();
        assert!(!name.is_empty());
        let parts: Vec<&str> = name.split_whitespace().collect();
        assert_eq!(parts.len(), 2);
    }
}
