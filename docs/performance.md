# Performance notes

Numbers from `cargo bench -p foliopdf` on an Apple M-series laptop, single
thread, 200-page text document (115 KB uncompressed, 81 KB compressed).

| Operation | Time | Throughput |
|---|---|---|
| Load (xref table) | 0.47 ms | 235 MB/s |
| Load (object streams) | 0.54 ms | 146 MB/s |
| Load and decrypt AES-256 | 6.8 ms | 13 MB/s |
| Save uncompressed | 0.59 ms | 188 MB/s |
| Save compressed (level 6) | 6.9 ms | 11.5 MB/s |
| Save compressed and encrypted (AES-256) | 27 ms | 3.4 MB/s |
| Merge two 200-page documents | 0.92 ms | 172 MB/s |
| Recover a file with a destroyed xref | 1.0 ms | 111 MB/s |

Corpus: 452 real PDFs, 211 MB, load + compress + reload in 11.5 s (about
18 MB/s end to end, dominated by Flate).

## Where the time goes

- **Parsing** is a single pass over the bytes with no allocation for
  delimiters or numbers; the lexer is the hottest code and is written
  accordingly. Objects are parsed eagerly because editing tools touch most
  of them anyway, and it removes a whole class of borrow-checker friction
  from the API.
- **Flate** is the bulk of both compressed save and corpus time.
  `miniz_oxide` is within a small factor of zlib. Level 6 is the sweet spot;
  levels 9–10 cost 3–5× the time for a few percent.
- **AES-256 (revision 6)** requires a password hash of at least 64 rounds,
  each round AES-encrypting 64 copies of the password material. That is
  ~5 ms per hash and the writer computes four (user, owner, and the two
  intermediate keys), so encrypting costs ~20 ms regardless of file size.
  Opening costs one or two hashes. This is a property of the standard, not
  the implementation, and it is what makes brute-forcing passwords slow.
- **Deduplication** hashes the serialised form of every stream and font
  dictionary, repeated to a fixed point (rarely more than 2–3 passes). It is
  linear in output size and only runs when `compress` is on.
- **Merge** is a deep copy with a worklist; cost is proportional to the
  objects reachable from the imported pages, not to the size of the source
  document.

## WebAssembly

Expect roughly 1.5–2× the native times in current browsers. The `.wasm` is
about 1.5 MB; use a Web Worker for files over a few megabytes to keep the UI
responsive, and transfer the output `Uint8Array` rather than copying it.

## Memory

Peak memory is roughly: input size (streams kept encoded) + parsed
dictionaries (small) + output buffer. Compressing re-inflates one stream at
a time. Loading a 200 MB scanned PDF needs a little over 200 MB.

## What would make it faster

In rough order of payoff:

1. Multi-threaded Flate for save (rayon behind a feature flag; not available
   in single-threaded wasm).
2. A zlib-ng backend behind a feature flag for native builds.
3. Skipping recompression for streams that are already Flate at a high
   level (currently they are recompressed and the smaller result kept).
