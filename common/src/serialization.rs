pub mod serialize_duration_as_seconds {
    use chrono::Duration;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(duration.num_seconds())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let seconds: i64 = Deserialize::deserialize(deserializer)?;
        Ok(Duration::try_seconds(seconds).expect("valid duration"))
    }
}

pub mod serialize_optional_duration_as_seconds {
    use chrono::Duration;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(duration) => serializer.serialize_i64(duration.num_seconds()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let seconds: Option<i64> = Deserialize::deserialize(deserializer)?;
        match seconds {
            Some(seconds) => Ok(Some(
                Duration::try_seconds(seconds).expect("valid duration"),
            )),
            None => Ok(None),
        }
    }
}
