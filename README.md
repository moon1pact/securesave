# securesave
<a id="readme-top"></a>

[![Contributors][contributors-shield]][contributors-url]
[![Forks][forks-shield]][forks-url]
[![Stargazers][stars-shield]][stars-url]
[![Issues][issues-shield]][issues-url]
[![MIT License][license-shield]][license-url]

<br />
<div align="center">
  <h3 align="center">SecureSave</h3>

  <p align="center">
    A modern, reliable backup manager for Linux, written in Rust.
    <br />
    <br />
    <a href="https://github.com/moon1pact/securesave/issues/new?labels=bug">Report Bug</a>
    &middot;
    <a href="https://github.com/moon1pact/securesave/issues/new?labels=enhancement">Request Feature</a>
  </p>
</div>

> **Status : v1.0.** The core engine, the on-disk formats (plain mirror,
> `.zst` files, manifest v1) and the CLI are stable. As with any backup
> tool : test your restore path before you rely on it.

<details>
  <summary>Table of Contents</summary>
  <ol>
    <li>
      <a href="#about-the-project">About The Project</a>
      <ul>
        <li><a href="#features">Features</a></li>
        <li><a href="#built-with">Built With</a></li>
      </ul>
    </li>
    <li>
      <a href="#getting-started">Getting Started</a>
      <ul>
        <li><a href="#prerequisites">Prerequisites</a></li>
        <li><a href="#installation">Installation</a></li>
        <li><a href="#quick-start">Quick start</a></li>
      </ul>
    </li>
    <li><a href="#usage">Usage</a></li>
    <li><a href="#configuration">Configuration</a></li>
    <li><a href="#how-compressed-backups-work">How Compressed Backups Work</a></li>
    <li><a href="#current-limitations">Current Limitations</a></li>
    <li><a href="#roadmap">Roadmap</a></li>
    <li><a href="#contributing">Contributing</a></li>
    <li><a href="#license">License</a></li>
    <li><a href="#contact">Contact</a></li>
    <li><a href="#acknowledgments">Acknowledgments</a></li>
  </ol>
</details>

## About The Project

SecureSave favors **correctness over features**. Every file is written
through a temporary file, flushed to disk (`fsync`), then atomically renamed
into place : an interrupted backup never leaves a half-written file at its
final path. The project grows slowly and deliberately : each feature is added
only when its design is solid.

* **Reliable by construction** : atomic writes, timestamps preserved, errors
  always report the affected path
* **Recoverable without the tool** : plain backups are a browsable mirror;
  compressed backups are ordinary `.zst` files that standard `zstd -d` can
  restore. Your data never depends on SecureSave existing
* **Simple** : one binary, one TOML file, no daemon
* **Honest** : known limitations are documented below

<p align="right">(<a href="#readme-top">back to top</a>)</p>

### Features

* **Incremental backups** : a file is copied only if its size or modification
  time changed since the last run ("quick check", as rsync does)
* **Optional zstd compression** per job : every file is compressed
  individually and stored with a `.zst` suffix
* **Safe restore** : `securesave restore` detects the backup format
  automatically, verifies compressed backups against the manifest, and
  refuses to overwrite anything
* **Verification** : `securesave verify` reads every byte of a backup and
  reports all problems found (corruption, missing files, leftovers)
* **Status** : `securesave status` shows every job with its destination
  state and its last successful run. Everything is derived from the
  manifest; SecureSave keeps no other state
* **Local dashboard** : `securesave serve` shows the same status as a web
  page on `127.0.0.1`. Read-only, no JavaScript, never exposed to the
  network
* Directory trees, file permissions and file modification times are
  preserved. Symlinks are recreated as symlinks (never followed). Special
  files (sockets, FIFOs) are skipped and counted
* Named backup jobs in a TOML file, plus a direct mode for one-off backups

<p align="right">(<a href="#readme-top">back to top</a>)</p>

### Built With

* [![Rust][Rust-badge]][Rust-url]

Runtime dependencies are kept deliberately small : [clap][clap-url],
[serde][serde-url], [toml][toml-url], [zstd][zstd-url],
[serde_json][serde-json-url] and [tiny_http][tiny-http-url].

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Getting Started

### Prerequisites

SecureSave targets Linux and requires a stable Rust toolchain.

* rustup
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

### Installation

1. Clone the repo
   ```sh
   git clone https://github.com/moon1pact/securesave.git
   ```
2. Build and install the binary into `~/.cargo/bin`
   ```sh
   cd securesave
   cargo install --path .
   ```

### Quick start

Create `~/.config/securesave/config.toml`:

```toml
[jobs.documents]
source = "/home/you/Documents"
destination = "/mnt/backup/documents"
compression = "zstd"
```

Then :

```sh
securesave backup documents   # run the job (incremental after the first run)
securesave status             # see when each job last ran
securesave verify documents   # check the backup end to end
```

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Usage

### One-off backup

Copy a directory tree into a destination directory (always a plain,
uncompressed copy):

```console
$ securesave backup ~/Documents /mnt/backup/documents
Backup complete: 132 file(s) copied (5934012 bytes), 0 up to date, 3 symlink(s), 0 skipped
```

### Named jobs

Declare your recurring backups once in the configuration file, then run them
by name :

```console
$ securesave backup photos
Backup complete: 12 file(s) copied (48211394 bytes), 2829 up to date, 0 symlink(s), 0 skipped [compressed to 46102817 bytes]
```

Only the 12 files that changed since the previous run were copied; the
`[compressed to ...]` suffix appears for compressed jobs.

### Listing jobs

```console
$ securesave list
documents  /home/moon/Documents  -> /mnt/backup/documents
photos     /home/moon/Photos     -> /mnt/backup/photos
```

### Restoring

```console
$ securesave restore /mnt/backup/photos ~/Photos-restored
Restore complete: 2841 file(s) copied (1250781342 bytes), 0 up to date, 0 symlink(s), 0 skipped
```

Restore never overwrites anything : the target must not exist, or must be an
empty directory. The backup format (plain or compressed) is detected
automatically. For compressed backups, every file is verified against the
manifest during the restore : a missing or damaged file is a hard error,
while stray files (present in the backup but unknown to the manifest) are
skipped with a warning.

### Verifying

```console
$ securesave verify photos
Verify complete: 2841 file(s), 1250781342 bytes checked, 0 issue(s)
```

For compressed jobs, every file is fully decompressed (to nowhere) and
checked against the manifest; this reads every byte and detects on-disk
corruption, missing files and strays. For plain jobs, the backup is checked
structurally (every file readable, no leftover temporaries) and for
completeness against the source. Any issue is listed and the exit code is
non-zero.

To repair a corrupted file reported by `verify`, delete it from the backup
and run the job again. The incremental check trusts sizes and mtimes for
speed, so a plain re-run would not detect silent corruption on its own ;
deleting the damaged file forces it to be backed up afresh.

### Job status

```console
$ securesave status
documents  none  OK, last run unknown
photos     zstd  OK, last run 2 hour(s) ago, 2841 file(s)
```

The last-run time of compressed jobs comes from their manifest ; plain jobs
carry no state, so their last run is reported as unknown.

### Local dashboard

```console
$ securesave serve
Serving on http://127.0.0.1:7878 (Ctrl-C to stop)
```

Serves the job status as a self-refreshing HTML page. The dashboard is
**read-only** (no actions can be triggered from it) and binds to
`127.0.0.1` only : it is for the local user, never a network service. Use
`--port` to change the port.

### Exit codes

`0` success, `1` runtime error (or verification issues), `2` usage error.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Configuration

SecureSave reads `$XDG_CONFIG_HOME/securesave/config.toml`, falling back to
`~/.config/securesave/config.toml`. If the file does not exist, SecureSave
simply behaves as if no jobs were defined; direct mode works without any
configuration. Each job is a named entry :

```toml
# ~/.config/securesave/config.toml

[jobs.documents]
source = "/home/moon/Documents"
destination = "/mnt/backup/documents"

[jobs.photos]
source = "/home/moon/Photos"
destination = "/mnt/backup/photos"
compression = "zstd"   # optional; default: "none" (plain copy)
```

Notes :

* Paths must be **absolute**; `~` is not expanded inside the file
* Unknown fields are rejected with a clear error. A typo in an option should
  never silently turn into a backup that does not do what you think
* Future options will always be optional with sensible defaults : existing
  configuration files will keep working as SecureSave evolves

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## How Compressed Backups Work

With `compression = "zstd"`, every regular file is compressed individually :
`Photos/2024/img.jpg` becomes `img.jpg.zst` at the destination, under the
same directory structure. You can always restore by hand with `zstd -d`.

The destination also contains `.securesave/manifest.json`, which records the
size and mtime each source file had when it was backed up; this is what
incremental runs compare against, since compressed files cannot be compared
to the source directly. The manifest is written atomically at the end of a
successful run, and it is **never trusted alone**: if a `.zst` file is
missing from the destination, the file is backed up again regardless of what
the manifest says. Deleting the manifest simply forces a full re-backup.

On restore, the manifest doubles as an integrity check : each file's
decompressed size must match what was recorded at backup time, and every
listed file must be present.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Current Limitations

Documented deliberately : you should know exactly what the tool does and
does not do before trusting it.

* Files deleted from the source are **not** removed from the destination
  (and changing a job's `compression` setting does not clean up files
  written with the previous setting)
* Restore is all-or-nothing into a new or empty directory: no selective
  restore of a single file or subdirectory yet
* Direct mode (`backup <source> <destination>`) is always a plain copy;
  compression is only available for configured jobs
* Modification times of directories and symlinks are not preserved (regular
  files: yes)
* Compressed jobs require UTF-8 file names (a clear error otherwise)
* `verify` cannot check the *content* of plain backups (there is no
  manifest to compare against); it checks structure and completeness
* No encryption, no retention policies yet
* Linux only

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Roadmap

- [x] Reliable atomic copy engine
- [x] Incremental backups (size + mtime quick check)
- [x] Named jobs in a TOML configuration
- [x] Per-file zstd compression with an integrity manifest
- [x] Safe restore
- [x] `verify`, `status` and a local read-only dashboard
- [ ] Encryption of backups
- [ ] Retention policies (keep N versions)
- [ ] Scheduling via systemd timers
- [ ] Selective restore (a single file or subdirectory)
- [ ] Shell completions, packaging

See the [open issues][issues-url] for a full list of proposed features (and
known issues).

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Contributing

Contributions are what make the open source community such an amazing place
to learn, inspire, and create. Any contributions you make are **really
appreciated**.

Please keep in mind the project's philosophy : small, well-explained changes
with tests. For anything non-trivial, open an issue first to discuss the
design. Before submitting, make sure the following all pass :

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
```

1. Fork the Project
2. Create your Feature Branch (`git checkout -b feature/AmazingFeature`)
3. Commit your Changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the Branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## License

Distributed under the MIT License. See [`LICENSE`](LICENSE) for more
information.

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Contact

moon1pact ; [@moon1pact](https://github.com/moon1pact)

Project Link : [https://github.com/moon1pact/securesave](https://github.com/moon1pact/securesave)

<p align="right">(<a href="#readme-top">back to top</a>)</p>

## Acknowledgments

* [Zstandard](https://facebook.github.io/zstd/)
* [The Rust community](https://www.rust-lang.org/community)
* [Img Shields](https://shields.io)
* [Best-README-Template](https://github.com/othneildrew/Best-README-Template)

<p align="right">(<a href="#readme-top">back to top</a>)</p>

[contributors-shield]: https://img.shields.io/github/contributors/moon1pact/securesave.svg?style=for-the-badge
[contributors-url]: https://github.com/moon1pact/securesave/graphs/contributors
[forks-shield]: https://img.shields.io/github/forks/moon1pact/securesave.svg?style=for-the-badge
[forks-url]: https://github.com/moon1pact/securesave/network/members
[stars-shield]: https://img.shields.io/github/stars/moon1pact/securesave.svg?style=for-the-badge
[stars-url]: https://github.com/moon1pact/securesave/stargazers
[issues-shield]: https://img.shields.io/github/issues/moon1pact/securesave.svg?style=for-the-badge
[issues-url]: https://github.com/moon1pact/securesave/issues
[license-shield]: https://img.shields.io/github/license/moon1pact/securesave.svg?style=for-the-badge
[license-url]: https://github.com/moon1pact/securesave/blob/main/LICENSE
[Rust-badge]: https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white
[Rust-url]: https://www.rust-lang.org/
[clap-url]: https://crates.io/crates/clap
[serde-url]: https://crates.io/crates/serde
[toml-url]: https://crates.io/crates/toml
[zstd-url]: https://crates.io/crates/zstd
[serde-json-url]: https://crates.io/crates/serde_json
[tiny-http-url]: https://crates.io/crates/tiny_http
>>>>>>> 7b9174d (V1 of securesave !)
