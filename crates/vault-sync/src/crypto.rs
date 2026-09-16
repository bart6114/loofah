use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use libsodium_rs::{
    crypto_aead::xchacha20poly1305 as aead, crypto_kdf as kdf,
    crypto_secretstream::xchacha20poly1305 as stream, crypto_sign as sign,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::snapshot::{FileDigest, Manifest, Snapshot};
use crate::{Error, Result};

const FILE_MAGIC: &[u8; 8] = b"LFSFILE1";
const KIT_MAGIC: &[u8; 8] = b"LFSKIT01";
const CHUNK: usize = 1024 * 1024;
const MAX_MANIFEST: usize = 32 * 1024 * 1024;

pub struct VaultKey {
    vault: Uuid,
    root: kdf::Key,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileObject {
    pub object: Uuid,
    pub plaintext: FileDigest,
    pub ciphertext: FileDigest,
    pub wrapped_key: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptedManifest {
    pub vault: Uuid,
    pub manifest: Manifest,
    pub objects: BTreeMap<String, FileObject>,
}

pub struct PreparedVersion {
    pub revision: Uuid,
    pub manifest: EncryptedManifest,
    pub encrypted_manifest: Vec<u8>,
    pub new_objects: BTreeMap<Uuid, tempfile::TempPath>,
}

impl EncryptedManifest {
    pub fn membership(&self) -> BTreeMap<Uuid, FileDigest> {
        self.objects
            .values()
            .map(|object| (object.object, object.ciphertext.clone()))
            .collect()
    }
    pub fn validate(&self) -> Result<()> {
        self.manifest.validate()?;
        if self.manifest.files.len() != self.objects.len() {
            return Err(Error::Authentication);
        }
        let mut identities = std::collections::BTreeSet::new();
        for (name, digest) in &self.manifest.files {
            let object = self.objects.get(name).ok_or(Error::Authentication)?;
            if &object.plaintext != digest
                || !identities.insert(object.object)
                || object.wrapped_key.len() != aead::NPUBBYTES + aead::ABYTES + stream::KEYBYTES
            {
                return Err(Error::Authentication);
            }
        }
        Ok(())
    }
}

impl VaultKey {
    pub fn generate(vault: Uuid) -> Result<Self> {
        libsodium_rs::ensure_init().map_err(|_| Error::Authentication)?;
        Ok(Self {
            vault,
            root: kdf::Key::generate().map_err(|_| Error::Authentication)?,
        })
    }

    pub fn vault(&self) -> Uuid {
        self.vault
    }

    /// Secret recovery material. The caller must save and confirm the kit before upload.
    pub fn recovery_kit(&self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(88));
        bytes.extend_from_slice(KIT_MAGIC);
        bytes.extend_from_slice(self.vault.as_bytes());
        bytes.extend_from_slice(self.root.as_bytes());
        let checksum = Sha256::digest(&*bytes);
        bytes.extend_from_slice(&checksum);
        bytes
    }

    pub fn from_recovery_kit(bytes: &[u8], expected_vault: Uuid) -> Result<Self> {
        libsodium_rs::ensure_init().map_err(|_| Error::Authentication)?;
        if bytes.len() != 88
            || &bytes[..8] != KIT_MAGIC
            || &bytes[8..24] != expected_vault.as_bytes()
        {
            return Err(Error::Authentication);
        }
        let checksum = Sha256::digest(&bytes[..56]);
        if !libsodium_rs::utils::memcmp(&checksum, &bytes[56..]) {
            return Err(Error::Authentication);
        }
        Ok(Self {
            vault: expected_vault,
            root: kdf::Key::from_slice(&bytes[24..56]).map_err(|_| Error::Authentication)?,
        })
    }

    pub fn enrollment_authority(&self) -> Result<sign::KeyPair> {
        sign::KeyPair::from_seed(&self.derive(3)?).map_err(|_| Error::Authentication)
    }

    pub fn entity_identity(&self, entity: &crate::scope::Entity) -> Result<String> {
        use libsodium_rs::crypto_auth::hmacsha256;
        entity.root()?;
        let key =
            hmacsha256::Key::from_bytes(&self.derive(4)?).map_err(|_| Error::Authentication)?;
        let payload = serde_json::to_vec(&("loofah-entity-v1", self.vault, entity))
            .map_err(|_| Error::Authentication)?;
        let digest = hmacsha256::auth(&payload, &key).map_err(|_| Error::Authentication)?;
        Ok(crate::enrollment::hex(&digest))
    }

    fn derive(&self, purpose: u64) -> Result<Zeroizing<Vec<u8>>> {
        kdf::derive_from_key(32, purpose, b"LFSYNC01", &self.root)
            .map(Zeroizing::new)
            .map_err(|_| Error::Authentication)
    }

    fn aad(&self, domain: &[u8; 8], identity: Uuid) -> Vec<u8> {
        [
            domain.as_slice(),
            self.vault.as_bytes(),
            identity.as_bytes(),
        ]
        .concat()
    }

    fn seal(
        &self,
        purpose: u64,
        domain: &[u8; 8],
        identity: Uuid,
        plaintext: &[u8],
    ) -> Result<Vec<u8>> {
        let key =
            aead::Key::from_bytes(&self.derive(purpose)?).map_err(|_| Error::Authentication)?;
        let nonce = aead::Nonce::generate();
        let ciphertext = aead::encrypt(plaintext, Some(&self.aad(domain, identity)), &nonce, &key)
            .map_err(|_| Error::Authentication)?;
        Ok([nonce.as_bytes().as_slice(), &ciphertext].concat())
    }

    fn open(
        &self,
        purpose: u64,
        domain: &[u8; 8],
        identity: Uuid,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        if ciphertext.len() < aead::NPUBBYTES + aead::ABYTES {
            return Err(Error::Authentication);
        }
        let key =
            aead::Key::from_bytes(&self.derive(purpose)?).map_err(|_| Error::Authentication)?;
        let nonce = aead::Nonce::try_from_slice(&ciphertext[..aead::NPUBBYTES])
            .map_err(|_| Error::Authentication)?;
        aead::decrypt(
            &ciphertext[aead::NPUBBYTES..],
            Some(&self.aad(domain, identity)),
            &nonce,
            &key,
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::Authentication)
    }

    pub fn seal_manifest(&self, revision: Uuid, manifest: &EncryptedManifest) -> Result<Vec<u8>> {
        manifest.validate()?;
        if manifest.vault != self.vault {
            return Err(Error::Authentication);
        }
        let plaintext =
            Zeroizing::new(serde_json::to_vec(manifest).map_err(|_| Error::Authentication)?);
        if plaintext.len() > MAX_MANIFEST {
            return Err(Error::Invalid("manifest too large".into()));
        }
        self.seal(1, b"LFSMETA1", revision, &plaintext)
    }

    pub fn open_manifest(&self, revision: Uuid, ciphertext: &[u8]) -> Result<EncryptedManifest> {
        if ciphertext.len() > MAX_MANIFEST + aead::NPUBBYTES + aead::ABYTES {
            return Err(Error::Authentication);
        }
        let plaintext = self.open(1, b"LFSMETA1", revision, ciphertext)?;
        let manifest: EncryptedManifest =
            serde_json::from_slice(&plaintext).map_err(|_| Error::Authentication)?;
        manifest.validate()?;
        if manifest.vault != self.vault {
            return Err(Error::Authentication);
        }
        // Our protocol uses one canonical encoding. This also rejects duplicate JSON keys.
        if serde_json::to_vec(&manifest)
            .map_err(|_| Error::Authentication)?
            .as_slice()
            != plaintext.as_slice()
        {
            return Err(Error::Authentication);
        }
        Ok(manifest)
    }

    pub fn prepare_version(
        &self,
        snapshot: &Snapshot,
        previous: Option<&EncryptedManifest>,
        spool: &Path,
    ) -> Result<PreparedVersion> {
        if let Some(previous) = previous {
            previous.validate()?;
            if previous.vault != self.vault
                || previous.manifest.entity != snapshot.manifest().entity
            {
                return Err(Error::Authentication);
            }
        }
        let mut objects = BTreeMap::new();
        let mut new_objects = BTreeMap::new();
        for (name, digest) in &snapshot.manifest().files {
            if let Some(existing) = previous
                .and_then(|previous| previous.objects.get(name))
                .filter(|object| &object.plaintext == digest)
            {
                self.open(2, b"LFSWRAP1", existing.object, &existing.wrapped_key)?;
                objects.insert(name.clone(), existing.clone());
            } else {
                let mut input = std::fs::File::open(snapshot.directory().join(name))?;
                let (object, file) = self.encrypt_file(&mut input, digest, spool)?;
                new_objects.insert(object.object, file.into_temp_path());
                objects.insert(name.clone(), object);
            }
        }
        let manifest = EncryptedManifest {
            vault: self.vault,
            manifest: snapshot.manifest().clone(),
            objects,
        };
        let revision = Uuid::new_v4();
        let encrypted_manifest = self.seal_manifest(revision, &manifest)?;
        Ok(PreparedVersion {
            revision,
            manifest,
            encrypted_manifest,
            new_objects,
        })
    }

    pub fn restore_snapshot(
        &self,
        revision: Uuid,
        ciphertext: &[u8],
        membership: &BTreeMap<Uuid, FileDigest>,
        spool: &Path,
        mut fetch: impl FnMut(Uuid) -> Result<std::fs::File>,
    ) -> Result<Snapshot> {
        let encrypted = self.open_manifest(revision, ciphertext)?;
        if &encrypted.membership() != membership {
            return Err(Error::Authentication);
        }
        let directory = tempfile::Builder::new()
            .prefix("loofah-incoming-")
            .tempdir_in(spool)?;
        for (name, object) in &encrypted.objects {
            let destination = directory.path().join(name);
            std::fs::create_dir_all(destination.parent().unwrap())?;
            let input = &mut fetch(object.object)?;
            let file = self.decrypt_file(input, object, directory.path())?;
            file.persist_noclobber(destination).map_err(|e| e.error)?;
        }
        Snapshot::verified(encrypted.manifest, directory)
    }

    /// Persist the returned ciphertext unchanged for all retries. Losing it requires a
    /// fresh call (new object id, key and stream), never reconstructing multipart bytes.
    pub fn encrypt_file(
        &self,
        input: &mut impl Read,
        expected: &FileDigest,
        spool: &Path,
    ) -> Result<(FileObject, tempfile::NamedTempFile)> {
        let ciphertext_bound = expected
            .bytes
            .saturating_add(
                expected
                    .bytes
                    .div_ceil(CHUNK as u64)
                    .saturating_add(1)
                    .saturating_mul((stream::ABYTES + 4) as u64),
            )
            .saturating_add(32);
        if fs2::available_space(spool)? < ciphertext_bound.saturating_add(16 * 1024 * 1024) {
            return Err(Error::DiskFull);
        }
        let object = Uuid::new_v4();
        let key = stream::Key::generate();
        let wrapped_key = self.seal(2, b"LFSWRAP1", object, key.as_bytes())?;
        let (mut push, header) =
            stream::PushState::init_push(&key).map_err(|_| Error::Authentication)?;
        let aad = self.aad(b"LFSOBJ01", object);
        let mut file = tempfile::NamedTempFile::new_in(spool)?;
        let mut writer = DigestWriter::new(file.as_file_mut());
        writer.write_all(FILE_MAGIC)?;
        writer.write_all(&header)?;
        let mut plaintext_hash = Sha256::new();
        let mut plaintext_bytes = 0u64;
        let mut buffer = Zeroizing::new(vec![0; CHUNK]);
        loop {
            let mut count = 0;
            while count < buffer.len() {
                let read = input.read(&mut buffer[count..])?;
                if read == 0 {
                    break;
                }
                count += read;
            }
            if count == 0 {
                break;
            }
            plaintext_bytes = plaintext_bytes
                .checked_add(count as u64)
                .ok_or(Error::Authentication)?;
            if plaintext_bytes > expected.bytes {
                return Err(Error::Conflict);
            }
            plaintext_hash.update(&buffer[..count]);
            let encrypted = push
                .push(&buffer[..count], Some(&aad), stream::TAG_MESSAGE)
                .map_err(|_| Error::Authentication)?;
            write_frame(&mut writer, &encrypted)?;
        }
        if plaintext_bytes != expected.bytes || hex(&plaintext_hash.finalize()) != expected.sha256 {
            return Err(Error::Conflict);
        }
        write_frame(
            &mut writer,
            &push
                .push(&[], Some(&aad), stream::TAG_FINAL)
                .map_err(|_| Error::Authentication)?,
        )?;
        writer.flush()?;
        let ciphertext = writer.finish();
        file.as_file().sync_all()?;
        Ok((
            FileObject {
                object,
                plaintext: expected.clone(),
                ciphertext,
                wrapped_key,
            },
            file,
        ))
    }

    /// Plaintext is returned only after the complete stream, final tag, length and
    /// both digests verify. Failure drops the temporary file without installing it.
    pub fn decrypt_file(
        &self,
        input: &mut impl Read,
        object: &FileObject,
        staging: &Path,
    ) -> Result<tempfile::NamedTempFile> {
        if fs2::available_space(staging)? < object.plaintext.bytes.saturating_add(16 * 1024 * 1024)
        {
            return Err(Error::DiskFull);
        }
        let key = self.open(2, b"LFSWRAP1", object.object, &object.wrapped_key)?;
        let key = stream::Key::from_bytes(&key).map_err(|_| Error::Authentication)?;
        let mut reader = DigestReader::new(input);
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        if &magic != FILE_MAGIC {
            return Err(Error::Authentication);
        }
        let mut header = [0; stream::HEADERBYTES];
        reader.read_exact(&mut header)?;
        let mut pull =
            stream::PullState::init_pull(&header, &key).map_err(|_| Error::Authentication)?;
        let aad = self.aad(b"LFSOBJ01", object.object);
        let mut file = tempfile::NamedTempFile::new_in(staging)?;
        let mut writer = DigestWriter::new(file.as_file_mut());
        loop {
            let mut length = [0; 4];
            reader.read_exact(&mut length)?;
            let length = u32::from_be_bytes(length) as usize;
            if !(stream::ABYTES..=CHUNK + stream::ABYTES).contains(&length) {
                return Err(Error::Authentication);
            }
            let mut encrypted = vec![0; length];
            reader.read_exact(&mut encrypted)?;
            let (plaintext, tag) = pull
                .pull(&encrypted, Some(&aad))
                .map_err(|_| Error::Authentication)?;
            let plaintext = Zeroizing::new(plaintext);
            if tag == stream::TAG_FINAL {
                if !plaintext.is_empty() {
                    return Err(Error::Authentication);
                }
                break;
            }
            if tag != stream::TAG_MESSAGE || plaintext.is_empty() {
                return Err(Error::Authentication);
            }
            if writer.bytes.saturating_add(plaintext.len() as u64) > object.plaintext.bytes {
                return Err(Error::Authentication);
            }
            writer.write_all(&plaintext)?;
        }
        let mut extra = [0];
        if reader.read(&mut extra)? != 0 {
            return Err(Error::Authentication);
        }
        writer.flush()?;
        if writer.finish() != object.plaintext || reader.finish() != object.ciphertext {
            return Err(Error::Authentication);
        }
        file.as_file().sync_all()?;
        Ok(file)
    }
}

fn write_frame(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(bytes)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct DigestWriter<W> {
    inner: W,
    hash: Sha256,
    bytes: u64,
}

impl<W> DigestWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hash: Sha256::new(),
            bytes: 0,
        }
    }
    fn finish(self) -> FileDigest {
        FileDigest {
            bytes: self.bytes,
            sha256: hex(&self.hash.finalize()),
        }
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct DigestReader<R> {
    inner: R,
    hash: Sha256,
    bytes: u64,
}

impl<R> DigestReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hash: Sha256::new(),
            bytes: 0,
        }
    }
    fn finish(self) -> FileDigest {
        FileDigest {
            bytes: self.bytes,
            sha256: hex(&self.hash.finalize()),
        }
    }
}

impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn digest(bytes: &[u8]) -> FileDigest {
        FileDigest {
            bytes: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        }
    }

    #[test]
    fn complete_streams_roundtrip_including_empty_and_multichunk_files() {
        let root = VaultKey::generate(Uuid::new_v4()).unwrap();
        let spool = tempfile::tempdir().unwrap();
        for length in [0, 1, CHUNK, CHUNK * 2 + 71] {
            let bytes = vec![42; length];
            let (object, file) = root
                .encrypt_file(&mut bytes.as_slice(), &digest(&bytes), spool.path())
                .unwrap();
            let restored = root
                .decrypt_file(&mut File::open(file.path()).unwrap(), &object, spool.path())
                .unwrap();
            assert_eq!(std::fs::read(restored.path()).unwrap(), bytes);
        }
    }

    #[test]
    fn tamper_truncation_trailing_data_wrong_identity_and_wrong_root_fail() {
        let root = VaultKey::generate(Uuid::new_v4()).unwrap();
        let spool = tempfile::tempdir().unwrap();
        let bytes = vec![7; CHUNK + 4];
        let (object, file) = root
            .encrypt_file(&mut bytes.as_slice(), &digest(&bytes), spool.path())
            .unwrap();
        let ciphertext = std::fs::read(file.path()).unwrap();
        for offset in [0, 8, 32, ciphertext.len() / 2, ciphertext.len() - 1] {
            let mut bad = ciphertext.clone();
            bad[offset] ^= 1;
            assert!(
                root.decrypt_file(&mut bad.as_slice(), &object, spool.path())
                    .is_err()
            );
            assert!(
                root.decrypt_file(&mut &ciphertext[..offset], &object, spool.path())
                    .is_err()
            );
        }
        let mut trailing = ciphertext.clone();
        trailing.push(0);
        assert!(
            root.decrypt_file(&mut trailing.as_slice(), &object, spool.path())
                .is_err()
        );
        let mut swapped = object.clone();
        swapped.object = Uuid::new_v4();
        assert!(
            root.decrypt_file(&mut ciphertext.as_slice(), &swapped, spool.path())
                .is_err()
        );
        let wrong = VaultKey::generate(root.vault()).unwrap();
        assert!(
            wrong
                .decrypt_file(&mut ciphertext.as_slice(), &object, spool.path())
                .is_err()
        );
        let mut false_length = object.clone();
        false_length.plaintext.bytes -= 1;
        assert!(
            root.decrypt_file(&mut ciphertext.as_slice(), &false_length, spool.path())
                .is_err()
        );
    }

    #[test]
    fn recovery_kit_preserves_authority_and_is_bound_to_vault() {
        let vault = Uuid::new_v4();
        let original = VaultKey::generate(vault).unwrap();
        let mut kit = original.recovery_kit();
        let recovered = VaultKey::from_recovery_kit(&kit, vault).unwrap();
        let (original_public, _) = original.enrollment_authority().unwrap().into_tuple();
        let (recovered_public, _) = recovered.enrollment_authority().unwrap().into_tuple();
        assert_eq!(original_public.as_bytes(), recovered_public.as_bytes());
        assert!(VaultKey::from_recovery_kit(&kit, Uuid::new_v4()).is_err());
        kit[30] ^= 1;
        assert!(VaultKey::from_recovery_kit(&kit, vault).is_err());
    }
}
