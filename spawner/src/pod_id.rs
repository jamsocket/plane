use convert_base::Convert;
use uuid::Uuid;

const ALPHANUM: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const PREFIX: &str = "session-";

#[derive(Debug, Clone, PartialEq)]
pub struct PodId(String);

impl PodId {
    pub fn new() -> Self {
        let mut base = Convert::new(256, 36);
        let result = base.convert::<u8, u8>(Uuid::new_v4().as_bytes());
        let ch = result.into_iter().map(|ix| ALPHANUM[ix as usize] as char);

        PodId(ch.collect())
    }

    pub fn from_prefixed_name(name: &str) -> Option<Self> {
        if let Some(name) = name.strip_prefix(PREFIX) {
            Some(PodId(name.to_string()))
        } else {
            None
        }
    }

    pub fn prefixed_name(&self) -> String {
        format!("{}{}", PREFIX, self.0)
    }

    pub fn name(&self) -> String {
        self.0.clone()
    }
}
