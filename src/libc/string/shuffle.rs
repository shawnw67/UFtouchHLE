/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Fisher-Yates with buffered OS entropy and unbiased bounded sampling.

pub(super) fn shuffle(bytes: &mut [u8]) -> Result<(), getrandom::Error> {
    let mut buffer = [0u8; 64];
    let mut offset = buffer.len();
    shuffle_with(bytes, || {
        if offset == buffer.len() {
            getrandom::getrandom(&mut buffer)?;
            offset = 0;
        }
        let value = u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap());
        offset += 4;
        Ok(value)
    })
}

fn below<E>(limit: u32, next: &mut impl FnMut() -> Result<u32, E>) -> Result<u32, E> {
    assert!(limit != 0);
    let threshold = limit.wrapping_neg() % limit;
    loop {
        let value = next()?;
        if value >= threshold {
            return Ok(value % limit);
        }
    }
}

fn shuffle_with<E>(bytes: &mut [u8], mut next: impl FnMut() -> Result<u32, E>) -> Result<(), E> {
    for limit in (2..=bytes.len()).rev() {
        let index = below(
            u32::try_from(limit).expect("strfry length exceeds guest address space"),
            &mut next,
        )?;
        bytes.swap(limit - 1, index as usize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_sampling_handles_non_powers_of_two_and_maximum_limit() {
        let mut values = [0, 1, 2, 3].into_iter();
        let mut next = || values.next().ok_or(());
        assert_eq!(below(3, &mut next), Ok(1));
        assert_eq!(below(3, &mut next), Ok(2));
        assert_eq!(below(3, &mut next), Ok(0));
        let mut values = [0, u32::MAX].into_iter();
        assert_eq!(below(u32::MAX, &mut || values.next().ok_or(())), Ok(0));
    }

    #[test]
    fn injected_stream_drives_swaps_without_changing_the_multiset() {
        let mut text = *b"abcd";
        let mut values = [4, 3, 2].into_iter();
        assert_eq!(shuffle_with(&mut text, || values.next().ok_or(())), Ok(()));
        assert_eq!(&text, b"bcda");
        let mut second = *b"abcd";
        let mut values = [7, 5, 3].into_iter();
        assert_eq!(
            shuffle_with(&mut second, || values.next().ok_or(())),
            Ok(())
        );
        assert_eq!(&second, b"abcd");
        text.sort_unstable();
        assert_eq!(text, second);
    }

    #[test]
    fn short_strings_need_no_entropy_and_source_failures_are_not_success() {
        let mut never = || -> Result<u32, ()> { panic!("unexpected entropy read") };
        assert_eq!(shuffle_with(&mut [], &mut never), Ok(()));
        assert_eq!(shuffle_with(&mut [b'a'], &mut never), Ok(()));
        let mut text = *b"abcd";
        assert_eq!(
            shuffle_with(&mut text, || Err("entropy unavailable")),
            Err("entropy unavailable")
        );
        assert_eq!(&text, b"abcd");
        let mut values = [4].into_iter();
        assert_eq!(
            shuffle_with(&mut text, || values.next().ok_or("late entropy failure")),
            Err("late entropy failure")
        );
        text.sort_unstable();
        assert_eq!(&text, b"abcd");
    }
}
