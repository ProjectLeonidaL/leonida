//! Translated shaders kept on disk (decision 0003): under the configured
//! cache folder, keyed by the container's content and by the translator
//! that translated it (its source fingerprint and settings, see `build`), so
//! a changed translator never serves an old translation. Entries go through
//! `leonida-pipeline`'s content-addressed cache, which writes them
//! atomically; an entry that does not read back is translated again.
//! A new translation is returned directly, even if its entry cannot be
//! saved. A cache write failure warns once; it does not discard the shader.
//!
//! The entry is our own encoding of [`HostShader`]: a field added to it
//! must be added to `encode` and `decode` too (entries of older builds
//! are ignored anyway).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use leonida_pipeline::{Cache, Converter};
use leonida_shader::emit::{HostShader, ImageBinding, StaticSampler, UserData};
use leonida_shader::resource::RootIndexedView;
use leonida_shader::{HwStage, RootType};

static CACHE: OnceLock<Cache> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    match Cache::open(dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// What identifies the translation, besides the container: the shader
/// crate's fingerprint (its source, every switch it reads, the host facts the
/// device gave it, the loop limit, barrier waits and clean-up passes) and
/// this file's encoding of the result. Not the whole executable any more: a
/// rebuild that leaves the translator alone keeps its translations (each new
/// build used to translate the title's thousands of shaders again, which kept
/// the first minutes of every test run short of pipelines).
/// `LEONIDA_SHADER_CACHE_KEY=exe` restores the old key (the executable's hash).
/// Worked out per lookup, so a translation made before the device said its
/// wave size is never filed under the later key.
fn build() -> [u8; 32] {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let limit = leonida_shader::emit::loop_limit();
    if limit != leonida_shader::emit::DEFAULT_LOOP_LIMIT {
        WARNED.call_once(|| tracing::warn!(limit, "LEONIDA_SHADER_LOOP_LIMIT: translated loops stop sooner"));
    }
    static EXE: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    let exe = EXE.get_or_init(|| {
        if std::env::var_os("LEONIDA_SHADER_CACHE_KEY").is_some_and(|v| v == "exe") {
            match std::env::current_exe().and_then(std::fs::read) {
                Ok(bytes) => Some(*blake3::hash(&bytes).as_bytes()),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read our executable; keying on the translator");
                    None
                }
            }
        } else {
            None
        }
    });
    let mut h = blake3::Hasher::new();
    h.update(b"leonida shader cache key 2\0");
    if let Some(exe) = exe {
        h.update(exe);
    }
    h.update(&leonida_shader::emit::translation_fingerprint());
    h.update(include_bytes!("shader_cache.rs"));
    *h.finalize().as_bytes()
}

/// The offline warm-up (`leonida translate --warm`): `container` is
/// translated into the cache unless it is there already. `Ok(true)`: it
/// was translated now, but a cache write failure can leave it unsaved
/// (with a warning). Needs [`use_shader_cache`] and the same host facts
/// the run will have (`leonida_shader::emit::translation_fingerprint`).
pub fn warm(container: &[u8]) -> Result<bool, String> {
    let cache = CACHE.get().ok_or("no shader cache")?;
    let ran = AtomicBool::new(false);
    let translate = |c: &[u8]| {
        ran.store(true, Ordering::Relaxed);
        leonida_shader::emit::translate(c).map_err(|e| e.to_string())
    };
    cached(cache, &build(), container, translate).map(|_| ran.load(Ordering::Relaxed))
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    let translate = |c: &[u8]| leonida_shader::emit::translate(c).map_err(|e| e.to_string());
    match CACHE.get() {
        Some(cache) => cached(cache, &build(), container, translate),
        None => translate(container),
    }
}

/// The cache entry's converter: the build and the container in, the
/// encoded translation out. `made` keeps the result so a miss need not
/// read and decode the entry it just wrote.
struct Translate<F> {
    translate: F,
    made: Mutex<Option<HostShader>>,
}

impl<F: Fn(&[u8]) -> Result<HostShader, String> + Send + Sync> Converter for Translate<F> {
    fn id(&self) -> &'static str {
        "shader.dxil"
    }
    fn version(&self) -> u32 {
        // The build fingerprint in the input does the versioning.
        1
    }
    fn convert(&self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        let shader = (self.translate)(&input[32..]).map_err(anyhow::Error::msg)?;
        let bytes = encode(&shader);
        *self.made.lock().unwrap_or_else(|e| e.into_inner()) = Some(shader);
        Ok(bytes)
    }
}

fn cached(
    cache: &Cache,
    build: &[u8; 32],
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String> + Send + Sync,
) -> Result<HostShader, String> {
    let input = [&build[..], container].concat();
    let mut converter = Translate {
        translate,
        made: Mutex::new(None),
    };
    for _ in 0..2 {
        let entry = cache.get_or_convert(&converter, &input);
        if let Some(shader) = converter
            .made
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            MISSES.fetch_add(1, Ordering::Relaxed);
            match entry {
                Ok(path) => tracing::debug!(path = %path.display(), "shader translated and cached"),
                Err(error) => {
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        let key = leonida_pipeline::CacheKey::new(&converter, &input);
                        let path = cache.path_for(&key);
                        tracing::warn!(
                            path = %path.display(),
                            error = %format_args!("{error:#}"),
                            "shader not cached; check cache folder permissions and free space"
                        );
                    });
                }
            }
            return Ok(shader);
        }
        let path = entry.map_err(|e| format!("{e:#}"))?;
        if let Some(shader) = std::fs::read(&path).ok().and_then(|b| decode(&b)) {
            HITS.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(path = %path.display(), "shader from the cache");
            return Ok(shader);
        }
        tracing::debug!(path = %path.display(), "unreadable shader cache entry; translating again");
        let _ = std::fs::remove_file(&path);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    (converter.translate)(container)
}

const MAGIC: &[u8; 4] = b"VSH1";
const STAGES: [HwStage; 8] = [
    HwStage::Ls,
    HwStage::Hs,
    HwStage::Es,
    HwStage::Gs,
    HwStage::Vs,
    HwStage::Ps,
    HwStage::Ms,
    HwStage::Cs,
];

fn root_code(r: RootType) -> u32 {
    match r {
        RootType::Constants => 0,
        RootType::Descriptor => 1,
        RootType::Table => 2,
        RootType::ImmediateSampler => 3,
        RootType::ResourceHeap => 4,
        RootType::SamplerHeap => 5,
        RootType::Other(x) => 6 + x,
    }
}

fn root_type(code: u32) -> RootType {
    match code {
        0 => RootType::Constants,
        1 => RootType::Descriptor,
        2 => RootType::Table,
        3 => RootType::ImmediateSampler,
        4 => RootType::ResourceHeap,
        5 => RootType::SamplerHeap,
        x => RootType::Other(x - 6),
    }
}

/// Little-endian words and length-prefixed lists.
fn encode(s: &HostShader) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let word = |out: &mut Vec<u8>, v: u32| out.extend(v.to_le_bytes());
    let stage = STAGES.iter().position(|&x| x == s.stage).unwrap_or(0) as u32;
    word(&mut out, stage);
    word(&mut out, s.dxil.len() as u32);
    out.extend(&s.dxil);
    word(&mut out, s.user_data.len() as u32);
    for u in &s.user_data {
        for v in [u.index, u32::from(u.slot), root_code(u.root), u.dword] {
            word(&mut out, v);
        }
    }
    word(&mut out, s.samplers.len() as u32);
    for sampler in &s.samplers {
        for &v in sampler {
            word(&mut out, v);
        }
    }
    word(&mut out, s.static_samplers.len() as u32);
    for t in &s.static_samplers {
        let [u, v, w] = t.address;
        let words = [
            t.filter,
            u,
            v,
            w,
            t.mip_lod_bias.to_bits(),
            t.max_anisotropy,
            t.comparison,
            t.border,
            t.min_lod.to_bits(),
            t.max_lod.to_bits(),
        ];
        for v in words {
            word(&mut out, v);
        }
    }
    for slots in [&s.root_cbvs, &s.root_srvs, &s.root_uavs] {
        word(&mut out, slots.len() as u32);
        out.extend(slots);
    }
    word(&mut out, s.root_indexed_views.len() as u32);
    for view in &s.root_indexed_views {
        for value in [
            u32::from(view.table_slot),
            u32::from(view.constant_slot),
            view.constant_dword,
            view.byte_offset,
            u32::from(view.shift),
        ] {
            word(&mut out, value);
        }
    }
    match s.independent_waves {
        None => word(&mut out, 0),
        Some(waves) => {
            word(&mut out, 1);
            for n in waves.original_threads().into_iter().chain(waves.native_threads()) {
                word(&mut out, n);
            }
            word(&mut out, waves.wave_count());
        }
    }
    word(&mut out, s.image_bindings.len() as u32);
    for b in &s.image_bindings {
        for value in [b.word, u32::from(b.slot), b.dword, u32::from(b.uav)] {
            word(&mut out, value);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<HostShader> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let (head, rest) = self.0.split_at_checked(n)?;
            self.0 = rest;
            Some(head)
        }
        fn word(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn list(&mut self) -> Option<Vec<u8>> {
            let n = self.word()? as usize;
            Some(self.take(n)?.to_vec())
        }
    }
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return None;
    }
    let stage = *STAGES.get(r.word()? as usize)?;
    let dxil = r.list()?;
    let n = r.word()? as usize;
    let mut user_data = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        user_data.push(UserData {
            index: r.word()?,
            slot: u8::try_from(r.word()?).ok()?,
            root: root_type(r.word()?),
            dword: r.word()?,
        });
    }
    let n = r.word()? as usize;
    let mut samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        samplers.push([r.word()?, r.word()?, r.word()?, r.word()?]);
    }
    let n = r.word()? as usize;
    let mut static_samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        static_samplers.push(StaticSampler {
            filter: r.word()?,
            address: [r.word()?, r.word()?, r.word()?],
            mip_lod_bias: f32::from_bits(r.word()?),
            max_anisotropy: r.word()?,
            comparison: r.word()?,
            border: r.word()?,
            min_lod: f32::from_bits(r.word()?),
            max_lod: f32::from_bits(r.word()?),
        });
    }
    let root_cbvs = r.list()?;
    let root_srvs = r.list()?;
    let root_uavs = r.list()?;
    let n = r.word()? as usize;
    let mut root_indexed_views = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        root_indexed_views.push(RootIndexedView {
            table_slot: u8::try_from(r.word()?).ok()?,
            constant_slot: u8::try_from(r.word()?).ok()?,
            constant_dword: r.word()?,
            byte_offset: r.word()?,
            shift: u8::try_from(r.word()?).ok().filter(|&s| s < 32)?,
        });
    }
    let independent_waves = match r.word()? {
        0 => None,
        1 if stage == HwStage::Cs => {
            let original = [r.word()?, r.word()?, r.word()?];
            let native = [r.word()?, r.word()?, r.word()?];
            let count = r.word()?;
            let waves = leonida_shader::emit::IndependentWaves::new(original)?;
            if native != waves.native_threads() || count != waves.wave_count() {
                return None;
            }
            Some(waves)
        }
        _ => return None,
    };
    let n = r.word()? as usize;
    if n > 64 { return None; }
    let mut image_bindings = Vec::with_capacity(n);
    for _ in 0..n {
        image_bindings.push(ImageBinding {
            word: r.word()?,
            slot: u8::try_from(r.word()?).ok()?,
            dword: r.word()?,
            uav: match r.word()? { 0 => false, 1 => true, _ => return None },
        });
    }
    let shader = HostShader {
        stage,
        dxil,
        user_data,
        samplers,
        static_samplers,
        root_cbvs,
        root_srvs,
        root_uavs,
        root_indexed_views,
        independent_waves,
        image_bindings,
    };
    (r.0.is_empty() && shader.image_bindings_valid()).then_some(shader)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader() -> HostShader {
        HostShader {
            stage: HwStage::Ps,
            dxil: b"DXBC and the rest".to_vec(),
            user_data: vec![UserData {
                index: 8,
                slot: 1,
                root: RootType::Table,
                dword: 0,
            }],
            samplers: vec![[0x400, 0xc0_0000, 0x18f0_0000, 0]],
            static_samplers: vec![StaticSampler::from_words([
                0x400,
                0xc0_0000,
                0x18f0_0000,
                0,
            ])],
            root_cbvs: vec![2],
            root_srvs: vec![0, 3],
            root_uavs: vec![1],
            root_indexed_views: vec![RootIndexedView {
                table_slot: 1,
                constant_slot: 2,
                constant_dword: 3,
                byte_offset: 32,
                shift: 5,
            }],
            independent_waves: None,
            image_bindings: Vec::new(),
        }
    }

    #[test]
    fn image_binding_manifest_round_trips_and_rejects_malformed_words() {
        let mut value = shader();
        value.image_bindings = vec![ImageBinding { word: 9, slot: 1, dword: 8, uav: true }];
        assert_eq!(decode(&encode(&value)), Some(value.clone()));
        assert_eq!(value.user_data_dwords(), 12);
        for binding in [
            ImageBinding { word: 64, ..value.image_bindings[0] },
            ImageBinding { word: 8, ..value.image_bindings[0] },
            ImageBinding { slot: 64, ..value.image_bindings[0] },
            ImageBinding { slot: 2, ..value.image_bindings[0] },
            ImageBinding { dword: 1, ..value.image_bindings[0] },
            ImageBinding { dword: u32::MAX - 7, ..value.image_bindings[0] },
        ] {
            let mut bad = value.clone();
            bad.image_bindings[0] = binding;
            assert!(decode(&encode(&bad)).is_none());
        }
        for binding in [
            ImageBinding { word: 10, ..value.image_bindings[0] },
            ImageBinding { dword: 16, ..value.image_bindings[0] },
        ] {
            let mut bad = value.clone();
            bad.image_bindings.push(binding);
            assert!(decode(&encode(&bad)).is_none());
        }
        let bytes = encode(&value);
        for cut in 1..=20 { assert!(decode(&bytes[..bytes.len() - cut]).is_none()); }
        let mut bad = bytes.clone();
        let n = bad.len();
        bad[n - 4..].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode(&bad).is_none());
        bad[n - 20..n - 16].copy_from_slice(&65u32.to_le_bytes());
        assert!(decode(&bad).is_none());
    }

    #[test]
    fn independent_wave_execution_layout_survives_a_disk_cache_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let mut wanted = shader();
        wanted.stage = HwStage::Cs;
        wanted.independent_waves =
            leonida_shader::emit::IndependentWaves::new([8, 16, 1]);
        let first = cached(&cache, &[5; 32], b"authored layout", |_| Ok(wanted.clone())).unwrap();
        assert_eq!(first, wanted);
        let loaded = cached(&cache, &[5; 32], b"authored layout", |_| {
            panic!("an undamaged entry must load without translating")
        }).unwrap();
        assert_eq!(loaded, wanted, "cache loss must not silently dispatch the wrong geometry");
    }

    #[test]
    fn malformed_execution_layouts_cannot_be_loaded_from_cache() {
        let mut wanted = shader();
        wanted.stage = HwStage::Cs;
        wanted.independent_waves = leonida_shader::emit::IndependentWaves::new([8, 16, 1]);
        let valid = encode(&wanted);
        assert_eq!(decode(&valid), Some(wanted));
        // The tagged layout tail contains original and native dimensions,
        // then wave count. Corruption must not create a different dispatch.
        let tail = valid.len() - 32;
        for (offset, replacement) in [(tail, 2u32), (tail + 4, 0),
            (tail + 16, 32), (tail + 28, 3)]
        {
            let mut bad = valid.clone();
            bad[offset..offset + 4].copy_from_slice(&replacement.to_le_bytes());
            assert!(decode(&bad).is_none());
        }
        let mut graphics = valid.clone();
        let pixel = STAGES.iter().position(|&stage| stage == HwStage::Ps).unwrap() as u32;
        graphics[4..8].copy_from_slice(&pixel.to_le_bytes());
        assert!(decode(&graphics).is_none());
    }

    #[test]
    fn translations_come_back_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let build = [7u8; 32];
        let container = b"a console container".as_slice();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the second came from disk");

        // Another build translates again.
        assert_eq!(cached(&cache, &[8; 32], container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 2);

        // A damaged entry is a miss, translated and stored again.
        for e in walk(dir.path()) {
            std::fs::write(e, b"VSH1 broken").unwrap();
        }
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);

        // Failures are not stored.
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        assert!(cached(&cache, &build, b"other", fail).is_err());
    }

    #[test]
    fn a_translation_survives_an_unwritable_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let cache = Cache::open(&root).unwrap();
        std::fs::remove_dir(&root).unwrap();
        std::fs::write(&root, b"not a folder").unwrap();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };

        assert_eq!(cached(&cache, &[7; 32], b"a container", translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the result was kept");
        assert_eq!(std::fs::read(&root).unwrap(), b"not a folder");
    }

    #[test]
    fn a_translation_survives_failed_publication_without_leaving_a_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        let build = [7; 32];
        let container = b"a container";
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        let entries = walk(dir.path());
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        std::fs::remove_file(entry).unwrap();
        // A directory at the destination lets staging succeed but prevents publication.
        std::fs::create_dir(entry).unwrap();

        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 2, "one translation per miss");
        assert!(entry.is_dir(), "the conflicting destination was not removed");
        assert!(walk(dir.path()).is_empty(), "the staging file was removed");
    }

    #[test]
    fn translation_errors_are_reported_and_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Err::<HostShader, _>("synthetic translation refused".to_owned())
        };

        for expected in 1..=2 {
            let error = cached(&cache, &[7; 32], b"a container", translate).unwrap_err();
            assert!(error.contains("synthetic translation refused"), "{error}");
            assert_eq!(runs.load(Ordering::Relaxed), expected);
            assert!(walk(dir.path()).is_empty());
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                files.extend(walk(&e.path()));
            } else {
                files.push(e.path());
            }
        }
        files
    }

    #[test]
    fn entries_round_trip_and_reject_damage() {
        let bytes = encode(&shader());
        assert_eq!(decode(&bytes), Some(shader()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(decode(b"VSH0"), None);
    }
}
