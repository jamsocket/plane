use anyhow::anyhow;
use tiny_id::ShortCodeGenerator;
use uuid::Uuid;

pub enum NameGenerator {
    Uuid,
    ShortName(ShortCodeGenerator<char>),
}

impl Default for NameGenerator {
    fn default() -> Self {
        Self::Uuid
    }
}

impl NameGenerator {
    pub fn from_str(spec: &str) -> Result<Self, anyhow::Error> {
        if spec == "uuid" {
            Ok(NameGenerator::Uuid)
        } else if let Some(spec) = spec.strip_prefix("short:") {
            let (alphabet, length) = spec
                .split_once(':')
                .ok_or_else(|| anyhow!("Expected colon-separated alphabet descriptor and length after short:, e.g. short:alpha:4"))?;

            let length: usize = length
                .parse()
                .map_err(|_| anyhow!("Couldn't parse {} as positive int.", length))?;

            let generator = match alphabet {
                "alpha" => ShortCodeGenerator::new_uppercase(length),
                "numeric" => ShortCodeGenerator::new_numeric(length),
                "alphanum" => ShortCodeGenerator::new_alphanumeric(length),
                "alphanum-lower" => ShortCodeGenerator::new_lowercase_alphanumeric(length),
                _ => return Err(anyhow!("Unknown alphabet: {}. Valid alphabets: alpha, numeric, alphanum, alphanum-lower.", alphabet))
            };

            Ok(NameGenerator::ShortName(generator))
        } else {
            Err(anyhow!("Invalid name generator spec: {}", spec))
        }
    }

    pub fn generate(&mut self) -> String {
        match self {
            Self::Uuid => Uuid::new_v4().to_string(),
            Self::ShortName(gen) => gen.next_string(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::NameGenerator;

    #[test]
    fn test_from_str() {
        {
            let mut name_generator = NameGenerator::from_str("uuid").unwrap();
            let result = name_generator.generate();
            assert_eq!(36, result.len());
        }

        {
            let mut name_generator = NameGenerator::from_str("short:alpha:8").unwrap();
            let result = name_generator.generate();
            assert_eq!(8, result.len());
        }

        {
            let mut name_generator = NameGenerator::from_str("short:numeric:5").unwrap();
            let result = name_generator.generate();
            assert_eq!(5, result.len());
        }

        {
            let mut name_generator = NameGenerator::from_str("short:alphanum:8").unwrap();
            let result = name_generator.generate();
            assert_eq!(8, result.len());
        }

        {
            let mut name_generator = NameGenerator::from_str("short:alphanum-lower:4").unwrap();
            let result = name_generator.generate();
            assert_eq!(4, result.len());
        }
    }
}
