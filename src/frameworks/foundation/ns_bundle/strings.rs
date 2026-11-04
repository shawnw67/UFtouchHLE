/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! UTF-16 parser for the standard key/value `.strings` format.

type Entry = (Vec<u16>, Vec<u16>);

struct Parser<'a> {
    input: &'a [u16],
    position: usize,
}

impl Parser<'_> {
    fn next(&mut self) -> Option<u16> {
        let unit = *self.input.get(self.position)?;
        self.position += 1;
        Some(unit)
    }

    fn consume(&mut self, unit: u16) -> bool {
        if self.input.get(self.position) == Some(&unit) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn skip(&mut self) -> Option<()> {
        loop {
            while self
                .input
                .get(self.position)
                .is_some_and(|&u| char::from_u32(u.into()).is_some_and(char::is_whitespace))
            {
                self.position += 1;
            }
            let rest = &self.input[self.position..];
            if rest.starts_with(&[47, 47]) {
                self.position += 2;
                while self
                    .input
                    .get(self.position)
                    .is_some_and(|u| *u != 10 && *u != 13)
                {
                    self.position += 1;
                }
            } else if rest.starts_with(&[47, 42]) {
                self.position += 2;
                while !self.input[self.position..].starts_with(&[42, 47]) {
                    self.next()?;
                }
                self.position += 2;
            } else {
                return Some(());
            }
        }
    }

    fn string(&mut self) -> Option<Vec<u16>> {
        self.skip()?;
        let mut result = Vec::new();
        if !self.consume(34) {
            while let Some(&unit) = self.input.get(self.position) {
                if unit == 61
                    || unit == 59
                    || char::from_u32(unit.into()).is_some_and(char::is_whitespace)
                {
                    break;
                }
                if self.input[self.position..].starts_with(&[47, 47])
                    || self.input[self.position..].starts_with(&[47, 42])
                {
                    break;
                }
                result.push(self.next()?);
            }
            return (!result.is_empty()).then_some(result);
        }
        loop {
            match self.next()? {
                34 => return Some(result),
                92 => {
                    let escaped = self.next()?;
                    let unit = match escaped {
                        85 | 117 => {
                            let mut value = 0;
                            for _ in 0..4 {
                                value = value * 16
                                    + char::from_u32(self.next()?.into())?.to_digit(16)? as u16;
                            }
                            value
                        }
                        48..=55 => {
                            let mut value = escaped - 48;
                            for _ in 0..2 {
                                if let Some(&next @ 48..=55) = self.input.get(self.position) {
                                    value = value * 8 + next - 48;
                                    self.position += 1;
                                } else {
                                    break;
                                }
                            }
                            if value > 255 {
                                return None;
                            }
                            value
                        }
                        110 => 10,
                        114 => 13,
                        116 => 9,
                        98 => 8,
                        102 => 12,
                        10 => continue,
                        13 => {
                            self.consume(10);
                            continue;
                        }
                        other => other,
                    };
                    result.push(unit);
                }
                unit => result.push(unit),
            }
        }
    }
}

pub(super) fn parse(input: &[u16]) -> Option<Vec<Entry>> {
    let mut parser = Parser { input, position: 0 };
    let mut entries = Vec::new();
    loop {
        parser.skip()?;
        if parser.position == input.len() {
            return Some(entries);
        }
        let key = parser.string()?;
        parser.skip()?;
        if !parser.consume(61) {
            return None;
        }
        let value = parser.string()?;
        parser.skip()?;
        if !parser.consume(59) {
            return None;
        }
        entries.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_empty_escaped_unicode_and_comments() {
        let source: Vec<_> = r#"/* header */ "empty" = ""; // line
            "quoted" = "say \"hi\"\\\n";
            unicode = "\U00e9\UD83D\UDE00"; bare = value;"#
            .encode_utf16()
            .collect();
        let entries = parse(&source).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries[0].1.is_empty());
        assert_eq!(String::from_utf16(&entries[1].1).unwrap(), "say \"hi\"\\\n");
        assert_eq!(String::from_utf16(&entries[2].1).unwrap(), "é😀");
        assert_eq!(String::from_utf16(&entries[3].1).unwrap(), "value");
    }
    #[test]
    fn malformed_tables_return_failure() {
        for source in [
            "/* unterminated",
            "\"key\"=\"value\"",
            "\"key\"=\"unterminated",
            "\"key\"=\"\\Uxx00\";",
        ] {
            assert!(parse(&source.encode_utf16().collect::<Vec<_>>()).is_none());
        }
    }
}
