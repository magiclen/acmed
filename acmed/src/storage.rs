use crate::certificate::Certificate;
use crate::hooks::{self, FileStorageHookData, HookEnvData, HookType};
use acme_common::b64_encode;
use acme_common::crypto::{KeyPair, X509Certificate};
use acme_common::error::Error;
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone)]
enum FileType {
    AccountPrivateKey,
    AccountPublicKey,
    PrivateKey,
    Certificate,
}

impl fmt::Display for FileType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            FileType::AccountPrivateKey => "priv-key",
            FileType::AccountPublicKey => "pub-key",
            FileType::PrivateKey => "pk",
            FileType::Certificate => "crt",
        };
        write!(f, "{}", s)
    }
}

fn get_file_full_path(
    cert: &Certificate,
    file_type: FileType,
) -> Result<(String, String, PathBuf), Error> {
    let base_path = match file_type {
        FileType::AccountPrivateKey | FileType::AccountPublicKey => &cert.account_directory,
        FileType::PrivateKey => &cert.crt_directory,
        FileType::Certificate => &cert.crt_directory,
    };
    let file_name = match file_type {
        FileType::AccountPrivateKey | FileType::AccountPublicKey => format!(
            "{account}.{file_type}.{ext}",
            account = b64_encode(&cert.account.name),
            file_type = file_type.to_string(),
            ext = "pem"
        ),
        FileType::PrivateKey | FileType::Certificate => {
            // TODO: use cert.crt_name_format instead of a string literal
            format!(
                "{name}_{algo}.{file_type}.{ext}",
                name = cert.crt_name,
                algo = cert.algo.to_string(),
                file_type = file_type.to_string(),
                ext = "pem"
            )
        }
    };
    let mut path = PathBuf::from(&base_path);
    path.push(&file_name);
    Ok((base_path.to_string(), file_name, path))
}

fn get_file_path(cert: &Certificate, file_type: FileType) -> Result<PathBuf, Error> {
    let (_, _, path) = get_file_full_path(cert, file_type)?;
    Ok(path)
}

fn read_file(cert: &Certificate, path: &PathBuf) -> Result<Vec<u8>, Error> {
    cert.trace(&format!("Reading file {:?}", path));
    let mut file = File::open(path)?;
    let mut contents = vec![];
    file.read_to_end(&mut contents)?;
    Ok(contents)
}

#[cfg(unix)]
fn set_owner(cert: &Certificate, path: &PathBuf, file_type: FileType) -> Result<(), Error> {
    let (uid, gid) = match file_type {
        FileType::Certificate => (
            cert.cert_file_owner.to_owned(),
            cert.cert_file_group.to_owned(),
        ),
        FileType::PrivateKey => (cert.pk_file_owner.to_owned(), cert.pk_file_group.to_owned()),
        FileType::AccountPrivateKey | FileType::AccountPublicKey => {
            // The account private and public keys does not need to be accessible to users other different from the current one.
            return Ok(());
        }
    };
    let uid = match uid {
        Some(u) => {
            if u.bytes().all(|b| b.is_ascii_digit()) {
                let raw_uid = u.parse::<u32>().unwrap();
                let nix_uid = nix::unistd::Uid::from_raw(raw_uid);
                Some(nix_uid)
            } else {
                let user = nix::unistd::User::from_name(&u)?;
                user.map(|u| u.uid)
            }
        }
        None => None,
    };
    let gid = match gid {
        Some(g) => {
            if g.bytes().all(|b| b.is_ascii_digit()) {
                let raw_gid = g.parse::<u32>().unwrap();
                let nix_gid = nix::unistd::Gid::from_raw(raw_gid);
                Some(nix_gid)
            } else {
                let grp = nix::unistd::Group::from_name(&g)?;
                grp.map(|g| g.gid)
            }
        }
        None => None,
    };
    match uid {
        Some(u) => cert.trace(&format!("{:?}: setting the uid to {}", path, u.as_raw())),
        None => cert.trace(&format!("{:?}: uid unchanged", path)),
    };
    match gid {
        Some(g) => cert.trace(&format!("{:?}: setting the gid to {}", path, g.as_raw())),
        None => cert.trace(&format!("{:?}: gid unchanged", path)),
    };
    match nix::unistd::chown(path, uid, gid) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("{}", e).into()),
    }
}

fn write_file(cert: &Certificate, file_type: FileType, data: &[u8]) -> Result<(), Error> {
    let (file_directory, file_name, path) = get_file_full_path(cert, file_type.clone())?;
    let mut hook_data = FileStorageHookData {
        file_name,
        file_directory,
        file_path: path.to_owned(),
        env: HashMap::new(),
    };
    hook_data.set_env(&cert.env);
    let is_new = !path.is_file();

    if is_new {
        hooks::call(cert, &hook_data, HookType::FilePreCreate)?;
    } else {
        hooks::call(cert, &hook_data, HookType::FilePreEdit)?;
    }

    cert.trace(&format!("Writing file {:?}", path));
    let mut file = if cfg!(unix) {
        let mut options = OpenOptions::new();
        options.mode(match &file_type {
            FileType::Certificate => cert.cert_file_mode,
            FileType::PrivateKey => cert.pk_file_mode,
            FileType::AccountPublicKey => crate::DEFAULT_ACCOUNT_FILE_MODE,
            FileType::AccountPrivateKey => crate::DEFAULT_ACCOUNT_FILE_MODE,
        });
        options.write(true).create(true).open(&path)?
    } else {
        File::create(&path)?
    };
    file.write_all(data)?;
    if cfg!(unix) {
        set_owner(cert, &path, file_type)?;
    }

    if is_new {
        hooks::call(cert, &hook_data, HookType::FilePostCreate)?;
    } else {
        hooks::call(cert, &hook_data, HookType::FilePostEdit)?;
    }
    Ok(())
}

pub fn get_account_keypair(cert: &Certificate) -> Result<KeyPair, Error> {
    let path = get_file_path(cert, FileType::AccountPrivateKey)?;
    let raw_key = read_file(cert, &path)?;
    let key = KeyPair::from_pem(&raw_key)?;
    Ok(key)
}

pub fn set_account_keypair(cert: &Certificate, key_pair: &KeyPair) -> Result<(), Error> {
    let pem_pub_key = key_pair.private_key_to_pem()?;
    let pem_priv_key = key_pair.public_key_to_pem()?;
    write_file(cert, FileType::AccountPublicKey, &pem_priv_key)?;
    write_file(cert, FileType::AccountPrivateKey, &pem_pub_key)?;
    Ok(())
}

pub fn get_keypair(cert: &Certificate) -> Result<KeyPair, Error> {
    let path = get_file_path(cert, FileType::PrivateKey)?;
    let raw_key = read_file(cert, &path)?;
    let key = KeyPair::from_pem(&raw_key)?;
    Ok(key)
}

pub fn set_keypair(cert: &Certificate, key_pair: &KeyPair) -> Result<(), Error> {
    let data = key_pair.private_key_to_pem()?;
    write_file(cert, FileType::PrivateKey, &data)
}

pub fn get_certificate(cert: &Certificate) -> Result<X509Certificate, Error> {
    let path = get_file_path(cert, FileType::Certificate)?;
    let raw_crt = read_file(cert, &path)?;
    let crt = X509Certificate::from_pem(&raw_crt)?;
    Ok(crt)
}

pub fn write_certificate(cert: &Certificate, data: &[u8]) -> Result<(), Error> {
    write_file(cert, FileType::Certificate, data)
}

fn check_files(cert: &Certificate, file_types: &[FileType]) -> bool {
    for t in file_types.to_vec() {
        let path = match get_file_path(cert, t) {
            Ok(p) => p,
            Err(_) => {
                return false;
            }
        };
        cert.trace(&format!("Testing file path: {}", path.to_str().unwrap()));
        if !path.is_file() {
            return false;
        }
    }
    true
}

pub fn account_files_exists(cert: &Certificate) -> bool {
    let file_types = vec![FileType::AccountPrivateKey, FileType::AccountPublicKey];
    check_files(cert, &file_types)
}

pub fn certificate_files_exists(cert: &Certificate) -> bool {
    let file_types = vec![FileType::PrivateKey, FileType::Certificate];
    check_files(cert, &file_types)
}
