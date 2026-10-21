use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct BearerSecret(Arc<Zeroizing<String>>);

impl BearerSecret {
    pub fn from_string(value: String) -> Result<Self, String> {
        if value.len() < 43
            || value.len() > 256
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err("Bearer secret is invalid".into());
        }
        Ok(Self(Arc::new(Zeroizing::new(value))))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn expose_to_operator(&self) -> String {
        self.0.to_string()
    }
}

pub fn load_or_create_bearer_secret(path: &Path) -> Result<BearerSecret, String> {
    validate_secret_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 512 {
                return Err("Bearer secret file is unsafe".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.mode() & 0o077 != 0 {
                    return Err("Bearer secret file permissions are too broad".into());
                }
            }
            let mut bytes = Vec::new();
            File::open(path)
                .map_err(|_| "Bearer secret could not be opened")?
                .take(513)
                .read_to_end(&mut bytes)
                .map_err(|_| "Bearer secret could not be read")?;
            if bytes.len() > 512 {
                return Err("Bearer secret file is oversized".into());
            }
            let value = String::from_utf8(bytes)
                .map_err(|_| "Bearer secret is not UTF-8")?
                .trim()
                .to_owned();
            BearerSecret::from_string(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("Bearer secret path has no parent")?;
            fs::create_dir_all(parent)
                .map_err(|_| "Bearer secret directory could not be created")?;
            validate_secret_path(path)?;
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(|_| "Secure random generation failed")?;
            let encoded = URL_SAFE_NO_PAD.encode(random);
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(path)
                .map_err(|_| "Bearer secret could not be created")?;
            file.write_all(encoded.as_bytes())
                .and_then(|_| file.write_all(b"\n"))
                .and_then(|_| file.sync_all())
                .map_err(|_| "Bearer secret could not be persisted")?;
            BearerSecret::from_string(encoded)
        }
        Err(_) => Err("Bearer secret could not be inspected".into()),
    }
}

fn validate_secret_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Bearer secret path must be absolute".into());
    }
    let mut ancestors = path.ancestors().skip(1).collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Bearer secret directory ancestry is unsafe".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Bearer secret directory could not be inspected".into()),
        }
    }
    Ok(())
}
