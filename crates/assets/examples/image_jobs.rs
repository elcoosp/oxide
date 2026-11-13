//! The image job model walkthrough (§V2-C09): the bounded CPU lane, the
//! smaller AVIF lane, per-key single-flight, the CAS hooks and the retry
//! contract — the whole thing the images endpoint wires against fakes, so
//! the contract is readable without the server.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-assets --example image_jobs
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxide_fw_assets::image_jobs::{ImageCas, ImageJobs};

/// An in-memory stand-in for the tiered image store (the CLI adoption
/// feeds the real L0/L1 CAS here).
#[derive(Default)]
struct MemoryCas {
    entries: Mutex<BTreeMap<String, Arc<Vec<u8>>>>,
}

impl ImageCas for MemoryCas {
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.entries.lock().unwrap().get(key).cloned()
    }
    fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
        self.entries.lock().unwrap().insert(key.to_string(), bytes);
    }
}

/// A fake encoder that counts starts (the acceptance's "count via a fake
/// backend") and holds its lane for a beat so overlap is visible.
fn fake_encode(
    counter: Arc<AtomicUsize>,
    payload: &'static str,
) -> oxide_fw_assets::Result<Vec<u8>> {
    counter.fetch_add(1, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(3));
    Ok(payload.as_bytes().to_vec())
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("the walkthrough runtime builds");

    rt.block_on(async {
        // The default lanes: CPU = available_parallelism() - 1 (never 0),
        // AVIF = half of it (never 0) — the smaller lane the spec names.
        let cas = Arc::new(MemoryCas::default());
        let jobs = Arc::new(ImageJobs::new(Arc::clone(&cas)));
        println!("== the lanes ==");
        println!("  cpu lane permits: {}", jobs.cpu_permits());

        // One encode per burst: 8 concurrent requests for the same
        // variant single-flight into ONE job, every waiter gets the same
        // bytes, and the CAS receives the result.
        let encodes = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..8 {
            let jobs = Arc::clone(&jobs);
            let encodes = Arc::clone(&encodes);
            handles.push(tokio::spawn(async move {
                let key = "images/hero.avif@1024";
                let outcome = jobs
                    .run(key, true, {
                        let encodes = Arc::clone(&encodes);
                        move || fake_encode(encodes, "avif bytes")
                    })
                    .await
                    .expect("the shared encode answers");
                (i, outcome)
            }));
        }
        println!("\n== 8 concurrent requests for one variant ==");
        for h in handles {
            let (i, bytes) = h.await.unwrap();
            println!(
                "  waiter {i}: {} bytes (payload {:?})",
                bytes.len(),
                String::from_utf8_lossy(&bytes)
            );
        }
        println!(
            "  encodes started: {} (single-flight deduped the burst)",
            encodes.load(Ordering::SeqCst)
        );
        println!(
            "  CAS now holds the variant: {}",
            cas.get("images/hero.avif@1024").is_some()
        );

        // The resumability contract: a failed encode is not cached, the
        // next request retries, and the entry does not linger.
        let fails = Arc::new(AtomicUsize::new(1));
        let first = jobs
            .run("images/hero.webp@512", false, {
                let fails = Arc::clone(&fails);
                move || {
                    if fails.fetch_sub(1, Ordering::SeqCst) > 0 {
                        return Err(oxide_fw_assets::err("AST-IMG", "the fake encode failed"));
                    }
                    Ok(b"webp bytes".to_vec())
                }
            })
            .await;
        println!("\n== a failed encode ==");
        match first {
            Err(e) => println!("  first request: [{}] {}", e.0.code, e.0.message),
            Ok(_) => println!("  first request: unexpected success"),
        }
        println!(
            "  CAS after the failure: {} (failed encodes are not stored)",
            cas.get("images/hero.webp@512").is_some()
        );
        let second = jobs
            .run("images/hero.webp@512", false, {
                move || fake_encode(Arc::new(AtomicUsize::new(0)), "webp bytes")
            })
            .await
            .expect("the retry succeeds");
        println!(
            "  retry: {:?} — stored again",
            String::from_utf8_lossy(&second)
        );

        // A CAS hit never runs the job.
        let hit = jobs
            .run("images/hero.avif@1024", true, || {
                unreachable!("a CAS hit never runs the job")
            })
            .await
            .expect("the stored bytes answer");
        println!("\n== the warm path ==");
        println!(
            "  CAS hit: {} bytes, total encodes stayed {}",
            hit.len(),
            encodes.load(Ordering::SeqCst)
        );
    });
}
