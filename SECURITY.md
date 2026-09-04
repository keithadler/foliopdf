# Security

## Reporting

Email keith.adler@icloud.com with "foliopdf" in the subject. You will get
a reply within a few days. Please do not open a public issue for anything
that could let a crafted PDF cause harm until a fix is out.

## What counts

- A PDF that makes the parser panic, loop forever or allocate without bound
  (the crate promises errors, not crashes, on hostile input).
- Encryption or decryption producing output that a standards-conforming
  reader treats differently from what the API claims (for example a
  permission that is not actually enforced by the `/P` value written).
- Anything in the WASM bindings that lets page content reach JavaScript as
  code rather than data.

## What does not

- PDF permission flags being ignored by other software. `/P` is advisory by
  design of the PDF standard; only the user password provides confidentiality.
- Weakness of RC4 or AES-128 modes. They exist for compatibility; the default
  is AES-256 (revision 6) and the documentation says so.

## Design notes

- No `unsafe` code; the crate is `#![forbid(unsafe_code)]`.
- Nesting depth of objects is capped (256), image size is capped
  (200 megapixels), and the cross-reference chain is cycle-guarded.
- Random material (file identifiers, salts, IVs) comes from the operating
  system or the browser's `crypto.getRandomValues` via `getrandom`.
