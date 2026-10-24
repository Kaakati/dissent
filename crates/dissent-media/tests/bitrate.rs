//! The target audio bitrate reaches the audio pipeline through a callback during a real call.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{frame, memory_pair, MemoryTransport};
use dissent_media::{Call, MediaKey, Signal, MAX_KBPS, MIN_KBPS};
use tokio::time::{interval, timeout};

async fn run_call(mut call: Call<MemoryTransport>, run_for: Duration) -> Call<MemoryTransport> {
    call.wait_connected().await.unwrap();
    let end = tokio::time::sleep(run_for);
    tokio::pin!(end);
    let mut ticker = interval(Duration::from_millis(20));
    let mut seq = 0u64;
    loop {
        tokio::select! {
            _ = &mut end => break,
            _ = ticker.tick() => {
                call.send_audio_frame(&frame(seq)).await.unwrap();
                seq += 1;
            }
            next = call.next_frame() => {
                next.unwrap();
            }
        }
    }
    call
}

#[tokio::test]
async fn the_target_bitrate_reaches_the_pipeline_and_stays_in_the_opus_range() {
    let (caller_end, callee_end) = memory_pair();
    let key = MediaKey::new([17; 32]);
    let (mut caller, offer) = Call::offer_over(caller_end, key.clone(), 1).await.unwrap();
    let offer = Signal::from_bytes(&offer.to_bytes().unwrap()).unwrap();
    let (callee, answer) = Call::accept_over(callee_end, key, 2, offer).await.unwrap();
    caller.apply(answer).unwrap();

    let seen: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    caller.on_target_bitrate(move |kbps| sink.lock().unwrap().push(kbps));

    let run = Duration::from_secs(3);
    let (caller, _callee) = timeout(Duration::from_secs(30), async {
        tokio::join!(run_call(caller, run), run_call(callee, run))
    })
    .await
    .expect("the call did not finish in time");

    let values = seen.lock().unwrap().clone();
    println!(
        "target bitrate: {} changes, last {:?} kbps, estimate now {} kbps",
        values.len(),
        values.last(),
        caller.target_bitrate_kbps()
    );
    assert!(
        !values.is_empty(),
        "the callback never fired, so no RTT was measured"
    );
    assert!(values.iter().all(|v| (MIN_KBPS..=MAX_KBPS).contains(v)));
    assert!((MIN_KBPS..=MAX_KBPS).contains(&caller.target_bitrate_kbps()));
    assert!(caller.loss_fraction() >= 0.0);
}
