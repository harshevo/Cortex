//! Placing texts by the words they share, for when no embedding model has
//! given the episodes vectors.

/// Dimensions of the vectors.
const DIMENSIONS: usize = 256;

/// Counts words into a fixed number of buckets, with a sign per word so
/// that unrelated words cancel rather than pile up. Texts that share words
/// end up with similar vectors.
pub fn hash_words(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; DIMENSIONS];

    let words = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() > 2);

    for word in words {
        let hash = fnv1a(&word.to_lowercase());

        let bucket = (hash % DIMENSIONS as u64) as usize;
        let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };

        vector[bucket] += sign;
    }

    normalize(&mut vector);

    vector
}

fn fnv1a(word: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in word.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    hash
}

fn normalize(vector: &mut [f32]) {
    let length = vector.iter().map(|value| value * value).sum::<f32>().sqrt();

    if length > 0.0 {
        for value in vector.iter_mut() {
            *value /= length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn texts_that_share_words_are_closer() {
        let parser = hash_words("fix the parser crash on empty input");
        let parser_again = hash_words("parser crash when the input is empty");
        let unrelated = hash_words("add dark mode toggle to settings page");

        // The vectors have length one, so the dot product is the cosine.
        assert!(dot(&parser, &parser_again) > dot(&parser, &unrelated));
    }

    #[test]
    fn empty_text_is_a_zero_vector() {
        assert!(hash_words("").iter().all(|value| *value == 0.0));
    }
}
