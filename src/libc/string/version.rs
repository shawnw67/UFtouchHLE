/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Byte-based GNU version ordering, including its leading-zero convention.
//! Integer runs are compared by length, never parsed into fixed-size numbers.

pub(super) fn compare(left: &[u8], right: &[u8]) -> i32 {
    let byte = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(0);
    let mut offset = 0;
    let mut digit_start = 0;
    let mut only_zeroes = true;
    while byte(left, offset) == byte(right, offset) {
        let c = byte(left, offset);
        if c == 0 {
            return 0;
        }
        if !c.is_ascii_digit() {
            digit_start = offset + 1;
            only_zeroes = true;
        } else if c != b'0' {
            only_zeroes = false;
        }
        offset += 1;
    }
    let a = byte(left, offset);
    let b = byte(right, offset);
    let integral = |s: &[u8]| matches!(byte(s, digit_start), b'1'..=b'9');
    if integral(left) && integral(right) {
        let mut end = offset;
        while byte(left, end).is_ascii_digit() {
            if !byte(right, end).is_ascii_digit() {
                return 1;
            }
            end += 1;
        }
        if byte(right, end).is_ascii_digit() {
            return -1;
        }
    } else if only_zeroes && digit_start < offset && (a.is_ascii_digit() || b.is_ascii_digit()) {
        // After a shared all-zero prefix, another digit precedes a suffix
        // or the terminator, regardless of its ordinary byte ordering.
        return i32::from(a.wrapping_sub(b'0')) - i32::from(b.wrapping_sub(b'0'));
    }
    i32::from(a) - i32::from(b)
}

#[cfg(test)]
mod tests {
    use super::compare;

    #[test]
    fn leading_zero_order_and_non_digit_transitions() {
        let ordered = ["000", "00", "01", "010", "09", "0", "1", "9", "10"];
        for (i, a) in ordered.iter().enumerate() {
            for (j, b) in ordered.iter().enumerate() {
                assert_eq!(
                    compare(a.as_bytes(), b.as_bytes()).signum(),
                    i.cmp(&j) as i32
                );
            }
        }
        assert!(compare(b"a", b"1") > 0);
        assert!(compare(b"0/", b"01") > 0);
        assert!(compare(b"part1_f012", b"part1_f01") > 0);
        assert!(compare(b"item#99", b"item#100") < 0);
    }

    #[test]
    fn arbitrarily_long_numbers_and_non_utf8_bytes() {
        let long = vec![b'9'; 4096];
        let longer = vec![b'1'; 4097];
        assert!(compare(&long, &longer) < 0);
        assert!(compare(&long, b"2") > 0);
        assert_eq!(compare(&long, &long), 0);
        assert!(compare(b"\xff9", b"\xff10") < 0);
        assert!(compare(b"\xff", b"\x80") > 0);
    }

    #[test]
    fn matches_pinned_gnu_reference_fixtures() {
        fn decode(s: &str) -> Vec<u8> {
            if s == "-" {
                return Vec::new();
            }
            s.as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        }
        for line in include_str!("../../../tests/fixtures/strverscmp.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let fields: Vec<_> = line.split('\t').collect();
            let expected = fields[2].parse::<i32>().unwrap();
            let (a, b) = (decode(fields[0]), decode(fields[1]));
            assert_eq!(compare(&a, &b).signum(), expected, "{a:?} vs {b:?}");
        }
    }
}
