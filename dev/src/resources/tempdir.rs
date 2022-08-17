// use std::{path::PathBuf, env::temp_dir, fs::{create_dir_all, remove_dir_all}};
// use anyhow::Result;
// use crate::util::random_string;

// pub struct TemporaryDirectory {
//     pub path: PathBuf,
// }

// impl TemporaryDirectory {
//     pub fn new() -> Result<Self> {
//         let name = random_string(8);
//         let path = temp_dir().join(name);
//         create_dir_all(&path)?;
//         Ok(TemporaryDirectory {
//             path
//         })
//     }

//     pub fn path(&self) -> String {
//         self.path.to_str().unwrap().to_owned()
//     }
// }

// impl Drop for TemporaryDirectory {
//     fn drop(&mut self) {
//         remove_dir_all(&self.path).unwrap()
//     }
// }