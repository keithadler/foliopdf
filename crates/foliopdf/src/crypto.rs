//! Encryption and decryption using the standard security handler
//! (ISO 32000-1 §7.6 and ISO 32000-2 §7.6.4).
//!
//! Supported when **opening**: RC4 40–128 bit (R2/R3), AES-128 (R4/AESV2)
//! and AES-256 (R5/R6/AESV3), with user or owner passwords.
//!
//! Supported when **saving**: RC4-128, AES-128 and AES-256 (default).
//! AES-256 R6 is what Acrobat X and later produce and every modern viewer
//! opens; pick RC4 or AES-128 only when a legacy consumer requires it.

use aes::cipher::block_padding::{NoPadding, Pkcs7};
use aes::cipher::{BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Sha384, Sha512};

use crate::error::{Error, Result};
use crate::object::{Dict, Name, ObjRef, Object, PdfString};

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// Encryption algorithm to use when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Method {
    /// RC4 with a 128-bit key (revision 3). Legacy; weak.
    #[serde(rename = "rc4-128", alias = "rc4", alias = "rc4_128")]
    Rc4_128,
    /// AES-128 in CBC mode (revision 4). Widely compatible.
    #[serde(rename = "aes-128", alias = "aes128")]
    Aes128,
    /// AES-256 in CBC mode (revision 6, PDF 2.0). Recommended.
    #[default]
    #[serde(rename = "aes-256", alias = "aes256")]
    Aes256,
}

/// Document permissions granted to a user who opens the file with the user
/// password. All default to `true` (everything allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Permissions {
    /// Print the document.
    pub print: bool,
    /// Modify contents.
    pub modify: bool,
    /// Copy or extract text and graphics.
    pub copy: bool,
    /// Add or modify annotations and form fields.
    pub annotate: bool,
    /// Fill in existing form fields.
    pub fill_forms: bool,
    /// Extract text for accessibility.
    pub accessibility: bool,
    /// Insert, rotate or delete pages.
    pub assemble: bool,
    /// Print at full resolution.
    pub print_high_quality: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            print: true,
            modify: true,
            copy: true,
            annotate: true,
            fill_forms: true,
            accessibility: true,
            assemble: true,
            print_high_quality: true,
        }
    }
}

impl Permissions {
    /// Encodes the flags as the `/P` integer.
    pub fn to_p(&self) -> i32 {
        let mut p: u32 = 0xFFFF_F0C0; // bits 7,8 and 13–32 set; 1,2 clear
        let mut set = |bit: u32, on: bool| {
            if on {
                p |= 1 << (bit - 1);
            }
        };
        set(3, self.print);
        set(4, self.modify);
        set(5, self.copy);
        set(6, self.annotate);
        set(9, self.fill_forms);
        set(10, self.accessibility);
        set(11, self.assemble);
        set(12, self.print_high_quality);
        p as i32
    }
    /// Decodes a `/P` integer.
    pub fn from_p(p: i32) -> Self {
        let p = p as u32;
        let bit = |b: u32| p & (1 << (b - 1)) != 0;
        Self {
            print: bit(3),
            modify: bit(4),
            copy: bit(5),
            annotate: bit(6),
            fill_forms: bit(9),
            accessibility: bit(10),
            assemble: bit(11),
            print_high_quality: bit(12),
        }
    }
}

/// Settings for encrypting a document on save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct EncryptionOptions {
    /// Password required to open the document. Empty means anyone can open it
    /// (but the permissions still apply).
    pub user_password: String,
    /// Password that unlocks all permissions. Empty means the user password is
    /// used.
    pub owner_password: String,
    /// Algorithm. Defaults to AES-256.
    pub method: Method,
    /// Permissions for users opening with the user password.
    pub permissions: Permissions,
    /// Whether the XMP metadata stream is encrypted too. Default `true`.
    #[serde(default = "default_true")]
    pub encrypt_metadata: bool,
}

fn default_true() -> bool {
    true
}

impl EncryptionOptions {
    /// AES-256 with the given passwords and full permissions.
    pub fn new(user_password: &str, owner_password: &str) -> Self {
        Self {
            user_password: user_password.to_owned(),
            owner_password: owner_password.to_owned(),
            encrypt_metadata: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cipher {
    None,
    Rc4,
    AesV2,
    AesV3,
}

/// Holds the file key and decides how each object is transformed.
#[derive(Debug, Clone)]
pub struct SecurityHandler {
    key: Vec<u8>,
    strings: Cipher,
    streams: Cipher,
    revision: u8,
    /// Whether the `/Metadata` stream is encrypted.
    pub encrypt_metadata: bool,
    /// Whether the document was opened with the owner password (or has none).
    pub is_owner: bool,
    /// Permissions declared by the document.
    pub permissions: Permissions,
}

impl SecurityHandler {
    /// Authenticates against an `/Encrypt` dictionary. Tries `password` as a
    /// user password first, then as an owner password.
    pub fn open(encrypt: &Dict, id0: &[u8], password: &str) -> Result<Self> {
        let filter = encrypt.get("Filter").and_then(Object::as_name);
        if filter.map(|n| n != "Standard").unwrap_or(true) {
            return Err(Error::UnsupportedEncryption(format!(
                "security handler /{}",
                filter
                    .map(|n| n.as_str().into_owned())
                    .unwrap_or_else(|| "?".into())
            )));
        }
        let v = encrypt.get("V").and_then(Object::as_i64).unwrap_or(0);
        let r = encrypt.get("R").and_then(Object::as_i64).unwrap_or(2) as u8;
        let mut length_bits = encrypt.get("Length").and_then(Object::as_i64).unwrap_or(40) as usize;
        let p = encrypt.get("P").and_then(Object::as_i64).unwrap_or(-1) as i32;
        let o = encrypt
            .get("O")
            .and_then(Object::as_string)
            .map(|s| s.bytes.clone())
            .unwrap_or_default();
        let u = encrypt
            .get("U")
            .and_then(Object::as_string)
            .map(|s| s.bytes.clone())
            .unwrap_or_default();
        let encrypt_metadata = encrypt
            .get("EncryptMetadata")
            .and_then(Object::as_bool)
            .unwrap_or(true);

        let (mut strings, mut streams) = (Cipher::Rc4, Cipher::Rc4);
        if v >= 4 {
            let cf = encrypt.get("CF").and_then(Object::as_dict);
            let resolve_cf = |key: &str| -> Result<(Cipher, Option<usize>)> {
                let name = encrypt
                    .get(key)
                    .and_then(Object::as_name)
                    .map(|n| n.as_str().into_owned());
                let name = name.unwrap_or_else(|| "Identity".into());
                if name == "Identity" {
                    return Ok((Cipher::None, None));
                }
                let f = cf.and_then(|cf| cf.get(&name)).and_then(Object::as_dict);
                let cfm = f
                    .and_then(|f| f.get("CFM"))
                    .and_then(Object::as_name)
                    .map(|n| n.as_str().into_owned());
                let len = f
                    .and_then(|f| f.get("Length"))
                    .and_then(Object::as_i64)
                    .map(|l| l as usize);
                let c = match cfm.as_deref() {
                    Some("V2") => Cipher::Rc4,
                    Some("AESV2") => Cipher::AesV2,
                    Some("AESV3") => Cipher::AesV3,
                    Some("None") | None => Cipher::None,
                    Some(other) => {
                        return Err(Error::UnsupportedEncryption(format!(
                            "crypt filter /{other}"
                        )))
                    }
                };
                Ok((c, len))
            };
            let (s, l1) = resolve_cf("StrF")?;
            let (t, l2) = resolve_cf("StmF")?;
            strings = s;
            streams = t;
            if let Some(l) = l1.or(l2) {
                // /Length in a crypt filter is in bytes (or bits when > 40).
                length_bits = if l <= 40 { l * 8 } else { l };
            }
        }

        let key = if r >= 5 {
            let oe = encrypt
                .get("OE")
                .and_then(Object::as_string)
                .map(|s| s.bytes.clone())
                .unwrap_or_default();
            let ue = encrypt
                .get("UE")
                .and_then(Object::as_string)
                .map(|s| s.bytes.clone())
                .unwrap_or_default();
            let (key, is_owner) = authenticate_r6(r, password, &o, &u, &oe, &ue)?;
            return Ok(Self {
                key,
                strings: if strings == Cipher::Rc4 {
                    Cipher::AesV3
                } else {
                    strings
                },
                streams: if streams == Cipher::Rc4 {
                    Cipher::AesV3
                } else {
                    streams
                },
                revision: r,
                encrypt_metadata,
                is_owner,
                permissions: Permissions::from_p(p),
            });
        } else {
            let n = if r == 2 {
                5
            } else {
                (length_bits / 8).clamp(5, 16)
            };
            let (key, is_owner) =
                authenticate_legacy(r, n, password, &o, &u, p, id0, encrypt_metadata)?;
            (key, is_owner)
        };
        let (key, is_owner) = key;
        Ok(Self {
            key,
            strings,
            streams,
            revision: r,
            encrypt_metadata,
            is_owner,
            permissions: Permissions::from_p(p),
        })
    }

    /// Builds a handler and the matching `/Encrypt` dictionary for writing.
    pub fn for_writing(opts: &EncryptionOptions, id0: &[u8]) -> Result<(Self, Dict)> {
        let p = opts.permissions.to_p();
        let mut dict = Dict::new();
        dict.set("Filter", "Standard").set("P", p as i64);
        match opts.method {
            Method::Aes256 => {
                let mut key = vec![0u8; 32];
                random(&mut key)?;
                let mut salts = [0u8; 32];
                random(&mut salts)?;
                let (uvs, uks, ovs, oks) =
                    (&salts[0..8], &salts[8..16], &salts[16..24], &salts[24..32]);
                let upw = utf8_password(&opts.user_password);
                let opw_src = if opts.owner_password.is_empty() {
                    &opts.user_password
                } else {
                    &opts.owner_password
                };
                let opw = utf8_password(opw_src);
                let mut u = hash_2b(&upw, uvs, &[]);
                u.extend_from_slice(uvs);
                u.extend_from_slice(uks);
                let ikey = hash_2b(&upw, uks, &[]);
                let ue = aes256_cbc_nopad_encrypt(&ikey, &[0u8; 16], &key);
                let mut o = hash_2b(&opw, ovs, &u);
                o.extend_from_slice(ovs);
                o.extend_from_slice(oks);
                let okey = hash_2b(&opw, oks, &u);
                let oe = aes256_cbc_nopad_encrypt(&okey, &[0u8; 16], &key);
                let mut perms = [0u8; 16];
                perms[..4].copy_from_slice(&p.to_le_bytes());
                perms[4..8].copy_from_slice(&[0xFF; 4]);
                perms[8] = if opts.encrypt_metadata { b'T' } else { b'F' };
                perms[9..12].copy_from_slice(b"adb");
                random(&mut perms[12..16])?;
                let perms = aes256_ecb_encrypt_block(&key, &perms);
                let mut cf = Dict::new();
                cf.set(
                    "StdCF",
                    Dict::new()
                        .with("CFM", "AESV3")
                        .with("AuthEvent", "DocOpen")
                        .with("Length", 32),
                );
                dict.set("V", 5)
                    .set("R", 6)
                    .set("Length", 256)
                    .set("CF", cf)
                    .set("StmF", "StdCF")
                    .set("StrF", "StdCF")
                    .set("O", PdfString::hex(o))
                    .set("U", PdfString::hex(u))
                    .set("OE", PdfString::hex(oe))
                    .set("UE", PdfString::hex(ue))
                    .set("Perms", PdfString::hex(perms.to_vec()));
                if !opts.encrypt_metadata {
                    dict.set("EncryptMetadata", false);
                }
                let h = Self {
                    key,
                    strings: Cipher::AesV3,
                    streams: Cipher::AesV3,
                    revision: 6,
                    encrypt_metadata: opts.encrypt_metadata,
                    is_owner: true,
                    permissions: opts.permissions,
                };
                Ok((h, dict))
            }
            Method::Aes128 | Method::Rc4_128 => {
                let aes = opts.method == Method::Aes128;
                let r = if aes { 4 } else { 3 };
                let n = 16;
                let upad = pad_password(&opts.user_password);
                let opad = if opts.owner_password.is_empty() {
                    upad
                } else {
                    pad_password(&opts.owner_password)
                };
                let o = compute_o(r, n, &opad, &upad);
                let key = compute_key(r, n, &upad, &o, p, id0, opts.encrypt_metadata);
                let u = compute_u(r, &key, id0);
                dict.set("R", r as i64)
                    .set("Length", (n * 8) as i64)
                    .set("O", PdfString::hex(o.to_vec()))
                    .set("U", PdfString::hex(u.to_vec()));
                if aes {
                    let mut cf = Dict::new();
                    cf.set(
                        "StdCF",
                        Dict::new()
                            .with("CFM", "AESV2")
                            .with("AuthEvent", "DocOpen")
                            .with("Length", 16),
                    );
                    dict.set("V", 4)
                        .set("CF", cf)
                        .set("StmF", "StdCF")
                        .set("StrF", "StdCF");
                    if !opts.encrypt_metadata {
                        dict.set("EncryptMetadata", false);
                    }
                } else {
                    dict.set("V", 2);
                }
                let c = if aes { Cipher::AesV2 } else { Cipher::Rc4 };
                let h = Self {
                    key,
                    strings: c,
                    streams: c,
                    revision: r,
                    encrypt_metadata: opts.encrypt_metadata,
                    is_owner: true,
                    permissions: opts.permissions,
                };
                Ok((h, dict))
            }
        }
    }

    /// Security handler revision (`/R`).
    pub fn revision(&self) -> u8 {
        self.revision
    }

    /// Whether streams and strings are actually transformed (a document can
    /// declare `/Identity` crypt filters).
    pub fn is_effective(&self) -> bool {
        self.strings != Cipher::None || self.streams != Cipher::None
    }

    fn object_key(&self, cipher: Cipher, r: ObjRef) -> Vec<u8> {
        match cipher {
            Cipher::AesV3 | Cipher::None => self.key.clone(),
            Cipher::Rc4 | Cipher::AesV2 => {
                let mut h = Md5::new();
                h.update(&self.key);
                h.update(&r.num.to_le_bytes()[..3]);
                h.update(&r.gen.to_le_bytes()[..2]);
                if cipher == Cipher::AesV2 {
                    h.update([0x73, 0x41, 0x6C, 0x54]);
                }
                let digest = h.finalize();
                let n = (self.key.len() + 5).min(16);
                digest[..n].to_vec()
            }
        }
    }

    fn transform(&self, cipher: Cipher, data: &[u8], r: ObjRef, encrypt: bool) -> Result<Vec<u8>> {
        let key = self.object_key(cipher, r);
        match cipher {
            Cipher::None => Ok(data.to_vec()),
            Cipher::Rc4 => Ok(rc4(&key, data)),
            Cipher::AesV2 | Cipher::AesV3 => {
                if encrypt {
                    let mut iv = [0u8; 16];
                    random(&mut iv)?;
                    let mut out = iv.to_vec();
                    if cipher == Cipher::AesV2 {
                        out.extend(
                            Aes128CbcEnc::new_from_slices(&key, &iv)
                                .unwrap()
                                .encrypt_padded_vec_mut::<Pkcs7>(data),
                        );
                    } else {
                        out.extend(
                            Aes256CbcEnc::new_from_slices(&key, &iv)
                                .unwrap()
                                .encrypt_padded_vec_mut::<Pkcs7>(data),
                        );
                    }
                    Ok(out)
                } else {
                    if data.len() < 16 {
                        return Ok(Vec::new());
                    }
                    let (iv, body) = data.split_at(16);
                    let body = &body[..body.len() / 16 * 16];
                    let res = if cipher == Cipher::AesV2 {
                        Aes128CbcDec::new_from_slices(&key, iv)
                            .unwrap()
                            .decrypt_padded_vec_mut::<Pkcs7>(body)
                    } else {
                        Aes256CbcDec::new_from_slices(&key, iv)
                            .unwrap()
                            .decrypt_padded_vec_mut::<Pkcs7>(body)
                    };
                    match res {
                        Ok(v) => Ok(v),
                        // Bad padding: return the raw blocks; viewers do the same.
                        Err(_) => {
                            let mut buf = body.to_vec();
                            if cipher == Cipher::AesV2 {
                                let _ = Aes128CbcDec::new_from_slices(&key, iv)
                                    .unwrap()
                                    .decrypt_padded_mut::<NoPadding>(&mut buf);
                            } else {
                                let _ = Aes256CbcDec::new_from_slices(&key, iv)
                                    .unwrap()
                                    .decrypt_padded_mut::<NoPadding>(&mut buf);
                            }
                            Ok(buf)
                        }
                    }
                }
            }
        }
    }

    /// Decrypts stream data belonging to object `r`.
    pub fn decrypt_stream(&self, data: &[u8], r: ObjRef) -> Result<Vec<u8>> {
        self.transform(self.streams, data, r, false)
    }
    /// Encrypts stream data belonging to object `r`.
    pub fn encrypt_stream(&self, data: &[u8], r: ObjRef) -> Result<Vec<u8>> {
        self.transform(self.streams, data, r, true)
    }
    /// Decrypts a string belonging to object `r`.
    pub fn decrypt_string(&self, data: &[u8], r: ObjRef) -> Result<Vec<u8>> {
        self.transform(self.strings, data, r, false)
    }
    /// Encrypts a string belonging to object `r`.
    pub fn encrypt_string(&self, data: &[u8], r: ObjRef) -> Result<Vec<u8>> {
        self.transform(self.strings, data, r, true)
    }

    /// Recursively decrypts every string and stream inside `obj` (which is
    /// indirect object `r`). Streams with `/Type /XRef` are skipped, as is
    /// `/Metadata` when `EncryptMetadata` is false.
    pub fn decrypt_object(&self, obj: &mut Object, r: ObjRef) -> Result<()> {
        self.walk(obj, r, false)
    }

    /// Recursively encrypts every string and stream inside `obj`.
    pub fn encrypt_object(&self, obj: &mut Object, r: ObjRef) -> Result<()> {
        self.walk(obj, r, true)
    }

    fn walk(&self, obj: &mut Object, r: ObjRef, encrypt: bool) -> Result<()> {
        match obj {
            Object::String(s) => {
                s.bytes = self.transform(self.strings, &s.bytes, r, encrypt)?;
            }
            Object::Array(a) => {
                for o in a {
                    self.walk(o, r, encrypt)?;
                }
            }
            Object::Dict(d) => {
                for v in d.0.values_mut() {
                    self.walk(v, r, encrypt)?;
                }
            }
            Object::Stream(s) => {
                let ty = s
                    .dict
                    .get("Type")
                    .and_then(Object::as_name)
                    .map(|n| n.as_str().into_owned());
                let is_xref = ty.as_deref() == Some("XRef");
                let is_meta = ty.as_deref() == Some("Metadata");
                let has_identity_crypt = s.filters().iter().any(|f| f == "Crypt")
                    && !s
                        .dict
                        .get("DecodeParms")
                        .map(|p| format!("{p:?}").contains("StdCF"))
                        .unwrap_or(false);
                for v in s.dict.0.values_mut() {
                    self.walk(v, r, encrypt)?;
                }
                if !is_xref && (!is_meta || self.encrypt_metadata) && !has_identity_crypt {
                    s.data = self.transform(self.streams, &s.data, r, encrypt)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn random(buf: &mut [u8]) -> Result<()> {
    getrandom::getrandom(buf)
        .map_err(|e| Error::Malformed(format!("random source unavailable: {e}")))
}

/// RC4 stream cipher (symmetric).
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = [0; 256];
    for (i, v) in s.iter_mut().enumerate() {
        *v = i as u8;
    }
    let mut j: u8 = 0;
    let klen = key.len().max(1);
    for i in 0..256 {
        j = j
            .wrapping_add(s[i])
            .wrapping_add(if key.is_empty() { 0 } else { key[i % klen] });
        s.swap(i, j as usize);
    }
    let mut out = Vec::with_capacity(data.len());
    let (mut i, mut j) = (0u8, 0u8);
    for &b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        out.push(b ^ k);
    }
    out
}

fn pad_password(pw: &str) -> [u8; 32] {
    // Passwords for R2–R4 are Latin-1 bytes.
    let bytes: Vec<u8> = pw
        .chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect();
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out[n..].copy_from_slice(&PAD[..32 - n]);
    out
}

fn utf8_password(pw: &str) -> Vec<u8> {
    let mut b = pw.as_bytes().to_vec();
    b.truncate(127);
    b
}

/// Algorithm 2: file key from a padded user password.
fn compute_key(
    r: u8,
    n: usize,
    padded_pw: &[u8; 32],
    o: &[u8],
    p: i32,
    id0: &[u8],
    encrypt_metadata: bool,
) -> Vec<u8> {
    let mut h = Md5::new();
    h.update(padded_pw);
    h.update(&o[..o.len().min(32)]);
    h.update(p.to_le_bytes());
    h.update(id0);
    if r >= 4 && !encrypt_metadata {
        h.update([0xFF, 0xFF, 0xFF, 0xFF]);
    }
    let mut key = h.finalize().to_vec();
    if r >= 3 {
        for _ in 0..50 {
            key = Md5::digest(&key[..n]).to_vec();
        }
    }
    key.truncate(n);
    key
}

/// Algorithm 3: the `/O` value.
fn compute_o(r: u8, n: usize, owner_pad: &[u8; 32], user_pad: &[u8; 32]) -> [u8; 32] {
    let mut okey = Md5::digest(owner_pad).to_vec();
    if r >= 3 {
        for _ in 0..50 {
            okey = Md5::digest(&okey[..n]).to_vec();
        }
    }
    okey.truncate(n);
    let mut x = rc4(&okey, user_pad);
    if r >= 3 {
        for i in 1..=19u8 {
            let k: Vec<u8> = okey.iter().map(|b| b ^ i).collect();
            x = rc4(&k, &x);
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&x[..32]);
    out
}

/// Algorithms 4 and 5: the `/U` value.
fn compute_u(r: u8, key: &[u8], id0: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    if r == 2 {
        out.copy_from_slice(&rc4(key, &PAD));
        return out;
    }
    let mut h = Md5::new();
    h.update(PAD);
    h.update(id0);
    let mut x = rc4(key, &h.finalize());
    for i in 1..=19u8 {
        let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
        x = rc4(&k, &x);
    }
    out[..16].copy_from_slice(&x[..16]);
    out
}

#[allow(clippy::too_many_arguments)]
fn authenticate_legacy(
    r: u8,
    n: usize,
    password: &str,
    o: &[u8],
    u: &[u8],
    p: i32,
    id0: &[u8],
    encrypt_metadata: bool,
) -> Result<(Vec<u8>, bool)> {
    let check = |padded: &[u8; 32]| -> Option<Vec<u8>> {
        let key = compute_key(r, n, padded, o, p, id0, encrypt_metadata);
        let cu = compute_u(r, &key, id0);
        let cmp = if r == 2 { 32 } else { 16 };
        if u.len() >= cmp && cu[..cmp] == u[..cmp] {
            Some(key)
        } else {
            None
        }
    };
    // User password.
    if let Some(k) = check(&pad_password(password)) {
        // Also treat as owner if the owner password equals the user password.
        return Ok((k, password.is_empty() && o.is_empty()));
    }
    // Owner password (Algorithm 7).
    let mut okey = Md5::digest(pad_password(password)).to_vec();
    if r >= 3 {
        for _ in 0..50 {
            okey = Md5::digest(&okey[..n]).to_vec();
        }
    }
    okey.truncate(n);
    let mut x = o[..o.len().min(32)].to_vec();
    if r == 2 {
        x = rc4(&okey, &x);
    } else {
        for i in (0..=19u8).rev() {
            let k: Vec<u8> = okey.iter().map(|b| b ^ i).collect();
            x = rc4(&k, &x);
        }
    }
    let mut user_pad = [0u8; 32];
    user_pad[..x.len().min(32)].copy_from_slice(&x[..x.len().min(32)]);
    if let Some(k) = check(&user_pad) {
        return Ok((k, true));
    }
    Err(Error::WrongPassword)
}

/// Plain SHA-256 hash (revision 5).
fn hash_r5(pw: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(pw);
    h.update(salt);
    h.update(udata);
    h.finalize().to_vec()
}

/// Algorithm 2.B hardened hash (revision 6).
fn hash_2b(pw: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut k = hash_r5(pw, salt, udata);
    let mut i = 0usize;
    loop {
        let mut k1 = Vec::with_capacity(64 * (pw.len() + k.len() + udata.len()));
        for _ in 0..64 {
            k1.extend_from_slice(pw);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(udata);
        }
        let e = Aes128CbcEnc::new_from_slices(&k[..16], &k[16..32])
            .unwrap()
            .encrypt_padded_vec_mut::<NoPadding>(&k1);
        let m: u32 = e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3;
        k = match m {
            0 => Sha256::digest(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        i += 1;
        if i >= 64 && (*e.last().unwrap_or(&0) as usize) <= i - 32 {
            break;
        }
    }
    k.truncate(32);
    k
}

fn authenticate_r6(
    r: u8,
    password: &str,
    o: &[u8],
    u: &[u8],
    oe: &[u8],
    ue: &[u8],
) -> Result<(Vec<u8>, bool)> {
    if u.len() < 48 || o.len() < 48 || ue.len() < 32 || oe.len() < 32 {
        return Err(Error::malformed("truncated /U, /O, /UE or /OE"));
    }
    let pw = utf8_password(password);
    let hash = |pw: &[u8], salt: &[u8], udata: &[u8]| {
        if r == 5 {
            hash_r5(pw, salt, udata)
        } else {
            hash_2b(pw, salt, udata)
        }
    };
    // User
    if hash(&pw, &u[32..40], &[]) == u[..32] {
        let ikey = hash(&pw, &u[40..48], &[]);
        let key = aes256_cbc_nopad_decrypt(&ikey, &[0u8; 16], &ue[..32]);
        return Ok((key, false));
    }
    // Owner
    if hash(&pw, &o[32..40], &u[..48]) == o[..32] {
        let ikey = hash(&pw, &o[40..48], &u[..48]);
        let key = aes256_cbc_nopad_decrypt(&ikey, &[0u8; 16], &oe[..32]);
        return Ok((key, true));
    }
    Err(Error::WrongPassword)
}

fn aes256_cbc_nopad_encrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    Aes256CbcEnc::new_from_slices(key, iv)
        .unwrap()
        .encrypt_padded_vec_mut::<NoPadding>(data)
}

fn aes256_cbc_nopad_decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut buf = data.to_vec();
    let _ = Aes256CbcDec::new_from_slices(key, iv)
        .unwrap()
        .decrypt_padded_mut::<NoPadding>(&mut buf);
    buf
}

fn aes256_ecb_encrypt_block(key: &[u8], block: &[u8; 16]) -> [u8; 16] {
    use aes::cipher::generic_array::GenericArray;
    let c = aes::Aes256::new_from_slice(key).unwrap();
    let mut b = GenericArray::clone_from_slice(block);
    c.encrypt_block(&mut b);
    b.into()
}

/// Name of the crypt filter method, for diagnostics.
pub fn describe(encrypt: &Dict) -> String {
    let v = encrypt.get("V").and_then(Object::as_i64).unwrap_or(0);
    let r = encrypt.get("R").and_then(Object::as_i64).unwrap_or(0);
    let cfm = encrypt
        .get("CF")
        .and_then(Object::as_dict)
        .and_then(|cf| cf.get("StdCF"))
        .and_then(Object::as_dict)
        .and_then(|f| f.get("CFM"))
        .and_then(Object::as_name)
        .map(Name::as_str);
    match (v, cfm.as_deref()) {
        (5, _) => "AES-256".into(),
        (4, Some("AESV2")) => "AES-128".into(),
        (4, _) => format!("RC4 (V4, R{r})"),
        _ => {
            let bits = encrypt.get("Length").and_then(Object::as_i64).unwrap_or(40);
            format!("RC4-{bits}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_names_match_docs() {
        assert_eq!(
            serde_json::to_string(&Method::Aes256).unwrap(),
            "\"aes-256\""
        );
        assert_eq!(
            serde_json::to_string(&Method::Aes128).unwrap(),
            "\"aes-128\""
        );
        assert_eq!(
            serde_json::to_string(&Method::Rc4_128).unwrap(),
            "\"rc4-128\""
        );
        for (s, m) in [
            ("\"aes-256\"", Method::Aes256),
            ("\"aes256\"", Method::Aes256),
            ("\"aes128\"", Method::Aes128),
            ("\"rc4\"", Method::Rc4_128),
        ] {
            assert_eq!(serde_json::from_str::<Method>(s).unwrap(), m);
        }
    }

    #[test]
    fn rc4_vector() {
        // Wikipedia test vector: Key "Key", Plaintext "Plaintext"
        let ct = rc4(b"Key", b"Plaintext");
        assert_eq!(ct, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
        assert_eq!(rc4(b"Key", &ct), b"Plaintext");
    }

    #[test]
    fn permissions_round_trip() {
        let p = Permissions {
            print: false,
            copy: false,
            ..Permissions::default()
        };
        let v = p.to_p();
        assert_eq!(Permissions::from_p(v), p);
        // Bits 1–2 are reserved and must be clear, so "everything allowed" is -4.
        assert_eq!(Permissions::default().to_p(), -4);
    }

    fn round_trip(method: Method, user: &str, owner: &str, open_with: &str) {
        let id0 = b"0123456789abcdef";
        let opts = EncryptionOptions {
            method,
            ..EncryptionOptions::new(user, owner)
        };
        let (h, dict) = SecurityHandler::for_writing(&opts, id0).unwrap();
        let r = ObjRef::new(7, 0);
        let ct = h.encrypt_string(b"secret text", r).unwrap();
        assert_ne!(ct, b"secret text");
        let opened = SecurityHandler::open(&dict, id0, open_with).unwrap();
        assert_eq!(opened.decrypt_string(&ct, r).unwrap(), b"secret text");
        let stream = h.encrypt_stream(&vec![7u8; 1000], r).unwrap();
        assert_eq!(opened.decrypt_stream(&stream, r).unwrap(), vec![7u8; 1000]);
        assert!(SecurityHandler::open(&dict, id0, "wrong-password").is_err());
    }

    #[test]
    fn aes256_user_and_owner() {
        round_trip(Method::Aes256, "user", "owner", "user");
        round_trip(Method::Aes256, "user", "owner", "owner");
        round_trip(Method::Aes256, "", "owner", "");
    }

    #[test]
    fn aes128_user_and_owner() {
        round_trip(Method::Aes128, "user", "owner", "user");
        round_trip(Method::Aes128, "user", "owner", "owner");
    }

    #[test]
    fn rc4_user_and_owner() {
        round_trip(Method::Rc4_128, "user", "owner", "user");
        round_trip(Method::Rc4_128, "user", "owner", "owner");
    }

    #[test]
    fn owner_flag() {
        let id0 = b"id";
        let (_, dict) =
            SecurityHandler::for_writing(&EncryptionOptions::new("u", "o"), id0).unwrap();
        assert!(!SecurityHandler::open(&dict, id0, "u").unwrap().is_owner);
        assert!(SecurityHandler::open(&dict, id0, "o").unwrap().is_owner);
    }
}
