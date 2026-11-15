<#
.SYNOPSIS
  Makes kiln-win extraction claim whole runs of files from one folder (the default now), instead
  of single files in archive order. Edits win\src\zip.rs in place.

.DESCRIPTION
  Run from the repo root (the folder that contains win\). It checks every anchor before it writes
  anything, keeps the file's line endings, and writes win\src\zip.rs.bak first.
  KILN_ORDER=archive and KILN_ORDER=rr still select the two older orders, for comparing.
#>
param([string]$Path = 'win\src\zip.rs')
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Path)) { throw "Not found: $Path (run this from the repo root)" }

$raw  = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $Path))
$crlf = $raw.Contains("`r`n")
$s    = $raw.Replace("`r`n", "`n")
if ($s.Contains('fn chunk_by_dir')) { Write-Host 'Already applied.'; return }

function Norm([string]$t) { return $t.Replace("`r`n", "`n") }
function Swap([string]$text, [string]$old, [string]$new, [string]$what) {
    $n = ([regex]::Matches($text, [regex]::Escape($old))).Count
    if ($n -ne 1) { throw "Anchor '$what' found $n times (expected 1): zip.rs differs from what I patched against. Send me the file." }
    return $text.Replace($old, $new)
}

$a1 = Norm @'
const T_CREATE: usize = 0;
'@
$b1 = Norm @'
/// A worker takes at most this many files from one folder before it claims another chunk.
const RUN_FILES: usize = 32;
/// ... or stops after this many bytes of file data, so folders of big files split up.
const RUN_BYTES: u64 = 4 << 20;

/// Cut `jobs` (in archive order) into chunks that each stay inside one folder, and order the chunks
/// so that chunk 0 of every folder comes first, then chunk 1 of every folder, and so on. Returns
/// the reordered jobs and each chunk's `(start, end)` range into them. A worker claims a whole
/// chunk at a time, so the workers that are busy at the same moment are in different folders (they
/// do not queue on one folder in the file system), and each still handles a run of neighbours.
fn chunk_by_dir(
    jobs: Vec<Job>,
    max_files: usize,
    max_bytes: u64,
    size: impl Fn(&Job) -> u64,
) -> (Vec<Job>, Vec<(usize, usize)>) {
    let total = jobs.len();
    let mut dir_ids: HashMap<PathBuf, usize> = HashMap::new();
    let mut folders: Vec<Vec<Vec<Job>>> = Vec::new();
    let mut open_bytes: Vec<u64> = Vec::new();
    for job in jobs {
        let parent = job.target.parent().unwrap_or(Path::new(""));
        let next_id = dir_ids.len();
        let dir = *dir_ids.entry(parent.to_path_buf()).or_insert(next_id);
        if dir == folders.len() {
            folders.push(Vec::new());
            open_bytes.push(0);
        }
        let bytes = size(&job);
        let runs = &mut folders[dir];
        let full = match runs.last() {
            Some(run) => run.len() >= max_files || open_bytes[dir] >= max_bytes,
            None => true,
        };
        if full {
            runs.push(vec![job]);
            open_bytes[dir] = bytes;
        } else if let Some(run) = runs.last_mut() {
            run.push(job);
            open_bytes[dir] += bytes;
        }
    }
    let mut out: Vec<Job> = Vec::with_capacity(total);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut iters: Vec<_> = folders.into_iter().map(Vec::into_iter).collect();
    loop {
        let mut any = false;
        for it in &mut iters {
            if let Some(run) = it.next() {
                any = true;
                let start = out.len();
                out.extend(run);
                ranges.push((start, out.len()));
            }
        }
        if !any {
            break;
        }
    }
    (out, ranges)
}

/// One chunk per job: workers claim files one at a time, in the order of `jobs`.
fn one_by_one(count: usize) -> Vec<(usize, usize)> {
    (0..count).map(|i| (i, i + 1)).collect()
}

const T_CREATE: usize = 0;
'@
$a2 = Norm @'
        jobs.sort_by_key(|j| self.entries[j.idx].local_off);
        if std::env::var("KILN_ORDER").is_ok_and(|v| v == "rr") {
            jobs = interleave_by_dir(jobs);
        }
        let prepass = began.elapsed();
        let trace = Trace::new();

        let nthreads = thread_count(opts.threads, jobs.len());
        let next = AtomicUsize::new(0);
'@
$b2 = Norm @'
        jobs.sort_by_key(|j| self.entries[j.idx].local_off);
        let nthreads = thread_count(opts.threads, jobs.len());
        // At least four chunks per worker, so a small or one-folder archive keeps them all busy.
        let run_files = (jobs.len() / (nthreads * 4)).clamp(1, RUN_FILES);
        // Default: a worker claims a run of files from one folder at a time. `KILN_ORDER=archive`
        // and `KILN_ORDER=rr` keep the older one-file-at-a-time orders, to compare against.
        let order = std::env::var("KILN_ORDER").unwrap_or_default();
        let (jobs, chunks) = match order.as_str() {
            "archive" => {
                let n = jobs.len();
                (jobs, one_by_one(n))
            }
            "rr" => {
                let jobs = interleave_by_dir(jobs);
                let n = jobs.len();
                (jobs, one_by_one(n))
            }
            _ => chunk_by_dir(jobs, run_files, RUN_BYTES, |j| self.entries[j.idx].size),
        };
        let prepass = began.elapsed();
        let trace = Trace::new();

        let next = AtomicUsize::new(0);
'@
$a4 = Norm @'
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(job) = jobs.get(k) else { break };
                            let e = &self.entries[job.idx];
                            if let Err(err) = self.extract_one(
                                e,
                                &job.target,
                                opts.overwrite,
                                &mut ctx,
                                &budget,
                                &abort,
                                &done,
                            ) {
                                abort.store(true, Ordering::Relaxed);
                                let mut slot = lock(&first_err);
                                if slot.is_none() {
                                    *slot = Some(err);
                                }
                                break;
                            }
'@
$b4 = Norm @'
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&(lo, hi)) = chunks.get(k) else { break };
                            for job in &jobs[lo..hi] {
                                if abort.load(Ordering::Relaxed) {
                                    break;
                                }
                                let e = &self.entries[job.idx];
                                if let Err(err) = self.extract_one(
                                    e,
                                    &job.target,
                                    opts.overwrite,
                                    &mut ctx,
                                    &budget,
                                    &abort,
                                    &done,
                                ) {
                                    abort.store(true, Ordering::Relaxed);
                                    let mut slot = lock(&first_err);
                                    if slot.is_none() {
                                        *slot = Some(err);
                                    }
                                    break;
                                }
                            }
'@
$tests = Norm @'
    fn jobs_for(paths: &[&str]) -> Vec<Job> {
        let mut jobs = Vec::new();
        for (idx, path) in paths.iter().enumerate() {
            jobs.push(Job {
                idx,
                target: PathBuf::from(path),
            });
        }
        jobs
    }

    /// Chunks stay inside one folder; chunk 0 of every folder comes first, then chunk 1, ...
    #[test]
    fn chunk_by_dir_cuts_runs_and_round_robins_folders() {
        let paths = ["a/1", "a/2", "a/3", "b/1", "a/4", "a/5", "b/2", "c/1"];
        let (jobs, chunks) = chunk_by_dir(jobs_for(&paths), 2, u64::MAX, |_| 1);
        let order: Vec<usize> = jobs.iter().map(|j| j.idx).collect();
        assert_eq!(order, [0, 1, 3, 6, 7, 2, 4, 5]);
        assert_eq!(chunks, [(0, 2), (2, 4), (4, 5), (5, 7), (7, 8)]);
    }

    /// A chunk also ends once it holds `max_bytes` of data, so big files do not pile up in one.
    #[test]
    fn chunk_by_dir_ends_a_chunk_at_the_byte_limit() {
        let paths = ["a/1", "a/2", "a/3", "a/4", "a/5"];
        let (jobs, chunks) = chunk_by_dir(jobs_for(&paths), 100, 10, |_| 6);
        assert_eq!(jobs.len(), 5);
        assert_eq!(chunks, [(0, 2), (2, 4), (4, 5)]);
    }
'@

$s = Swap $s ($a1.TrimEnd("`n") + "`n") ($b1.TrimEnd("`n") + "`n") 'const T_CREATE'
$s = Swap $s ($a2.TrimEnd("`n") + "`n") ($b2.TrimEnd("`n") + "`n") 'extract driver'
$s = Swap $s ($a4.TrimEnd("`n") + "`n") ($b4.TrimEnd("`n") + "`n") 'worker loop'

$m = $s.IndexOf("`nmod tests {")
if ($m -lt 0) { throw "Anchor 'mod tests' not found." }
$u = $s.IndexOf('use super::*;', $m)
if ($u -lt 0) { throw "Anchor 'use super::*;' not found in the tests module." }
$e = $s.IndexOf("`n", $u)
$s = $s.Substring(0, $e + 1) + "`n" + $tests.TrimEnd("`n") + "`n" + $s.Substring($e + 1)

if ($crlf) { $s = $s.Replace("`n", "`r`n") }
Copy-Item -LiteralPath $Path -Destination "$Path.bak" -Force
[IO.File]::WriteAllText((Resolve-Path -LiteralPath $Path), $s, (New-Object Text.UTF8Encoding $false))
Write-Host "Patched $Path (backup: $Path.bak). Commit, push, wait for CI."
