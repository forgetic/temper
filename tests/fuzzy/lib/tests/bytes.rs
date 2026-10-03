//! lib's byte search at random: against a naive one, over many haystacks and
//! needles of few letters, so that partial matches abound.

use temper_lib::Rng;
use temper_lib::bytes::{count, find_from};

/// The first occurrence at or after `from`, comparing at every position.
fn naive(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let last = haystack.len().checked_sub(needle.len())?;
    (from..=last).find(|at| haystack[*at..].starts_with(needle))
}

fn naive_count(haystack: &[u8], needle: &[u8], cap: u32) -> u32 {
    let mut found = 0;
    let mut from = 0;
    while found < cap {
        let Some(at) = naive(haystack, needle, from) else { break };
        found += 1;
        from = at + needle.len().max(1);
    }
    found
}

/// A length up to `max`, at random.
fn up_to(rng: &mut Rng, max: usize) -> usize {
    usize::try_from(rng.between(0, u64::try_from(max).expect("fits"))).expect("fits")
}

/// Fills `text` with letters from the first `letters` of the alphabet.
fn fill(rng: &mut Rng, text: &mut [u8], letters: u64) {
    for byte in text {
        *byte = b'a' + u8::try_from(rng.below(letters)).expect("a few");
    }
}

#[test]
fn the_search_agrees_with_a_naive_one() {
    let mut rng = Rng::new(0x5EA2_C4ED);
    let mut haystack_buffer = [0_u8; 64];
    let mut needle_buffer = [0_u8; 12];
    for round in 0_u32..20_000 {
        let letters = rng.between(1, 4);
        let len = up_to(&mut rng, haystack_buffer.len());
        fill(&mut rng, &mut haystack_buffer[..len], letters);
        let haystack = &haystack_buffer[..len];
        let needle: &[u8] = if rng.chance(500) && !haystack.is_empty() {
            // A piece of the haystack, so that it occurs at least once.
            let start = up_to(&mut rng, haystack.len() - 1);
            &haystack[start..start + 1 + up_to(&mut rng, haystack.len() - start - 1)]
        } else {
            let len = up_to(&mut rng, needle_buffer.len());
            fill(&mut rng, &mut needle_buffer[..len], letters);
            &needle_buffer[..len]
        };
        for from in 0..=haystack.len() + 1 {
            assert_eq!(find_from(haystack, needle, from), naive(haystack, needle, from), "round {round}");
        }
        for cap in [0, 1, 2, 3, u32::MAX] {
            assert_eq!(count(haystack, needle, cap), naive_count(haystack, needle, cap), "round {round}");
        }
    }
}
