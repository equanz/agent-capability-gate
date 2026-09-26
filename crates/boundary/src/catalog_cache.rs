//! Bounded, per-target catalog snapshots. A cache entry never selects a tool:
//! callers must intersect it with the administrator's active configuration.

use mcp_boundary_core::serde_json::{self, Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_CACHE_BYTES: u64 = 1024 * 1024;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub struct CachedTool {
    pub upstream: Value,
    pub public: Value,
}

#[derive(Clone, Debug)]
pub struct CatalogCache {
    directory: PathBuf,
    digest: String,
}

impl CatalogCache {
    pub fn new(config_path: &Path, config_bytes: &[u8]) -> io::Result<Self> {
        let parent = match std::env::var_os("XDG_CACHE_HOME") {
            Some(path) if Path::new(&path).is_absolute() => PathBuf::from(path),
            _ => {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
                PathBuf::from(home).join(".cache")
            }
        };
        let mut identity = config_path.as_os_str().as_encoded_bytes().to_vec();
        identity.push(0);
        identity.extend_from_slice(config_bytes);
        let digest = sha256_hex(&identity);
        Ok(Self {
            directory: parent.join("mcp-boundary").join("catalog-v1").join(&digest),
            digest: format!("sha256:{digest}"),
        })
    }

    pub fn load(&self, target: &str) -> Option<BTreeMap<String, CachedTool>> {
        let path = self.path(target)?;
        let file = File::open(path).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_CACHE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return None;
        }
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let object = value.as_object()?;
        if object.get("format_version")?.as_u64()? != 1
            || object.get("config_digest")?.as_str()? != self.digest
            || object.get("target")?.as_str()? != target
        {
            return None;
        }
        let tools = object.get("tools")?.as_object()?;
        if tools.len() > mcp_boundary_core::MAX_TOOLS {
            return None;
        }
        tools
            .iter()
            .map(|(name, entry)| {
                let entry = entry.as_object()?;
                let upstream = entry.get("upstream")?.as_object()?;
                let public = entry.get("public")?.as_object()?;
                Some((
                    name.clone(),
                    CachedTool {
                        upstream: Value::Object(upstream.clone()),
                        public: Value::Object(public.clone()),
                    },
                ))
            })
            .collect()
    }

    pub fn store(&self, target: &str, tools: &BTreeMap<String, CachedTool>) -> io::Result<()> {
        let destination = self.path(target).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid target identifier")
        })?;
        fs::create_dir_all(&self.directory)?;
        let data = json!({
            "format_version": 1,
            "config_digest": self.digest,
            "target": target,
            "tools": tools.iter().map(|(name, tool)| {
                (name.clone(), json!({"upstream": tool.upstream, "public": tool.public}))
            }).collect::<BTreeMap<_, _>>(),
        });
        let encoded = serde_json::to_vec(&data).map_err(io::Error::other)?;
        if encoded.len() as u64 > MAX_CACHE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog cache exceeds limit",
            ));
        }
        let temporary = self.directory.join(format!(
            ".{target}.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&temporary, destination)
    }

    fn path(&self, target: &str) -> Option<PathBuf> {
        if target.is_empty()
            || target.len() > 128
            || !target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || target == "."
            || target == ".."
        {
            return None;
        }
        Some(self.directory.join(format!("{target}.json")))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.as_chunks::<64>().0 {
        let mut words = [0u32; 64];
        for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            words[index] = u32::from_be_bytes(*chunk);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (word, update) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(update);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_matches_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
