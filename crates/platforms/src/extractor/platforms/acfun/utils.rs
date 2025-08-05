use rand::distr::{Alphanumeric, SampleString};

// Generates a random string of a given length.
pub fn get_random_name(n: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), n)
}
