//! Choosing which relay servers to list, and in what order. Pure functions, so the merge, the cap and
//! the ranking can be tested without a network. iroh picks the home relay itself from the latencies it
//! measures; these functions decide which relays it is given and how they are shown.

use std::time::Duration;

use url::Url;

/// Most relay servers the list keeps. The node also enforces this cap on relays that communities add.
pub const MAX_LIST: usize = 30;

/// Where a relay came from. Settings shows it, so a person can tell their own choices from ones a
/// community named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Added by this person in Settings.
    Custom,
    /// Named by a community's owners or admins.
    Community,
    /// number0's public relays.
    Public,
    /// Hosted by a member for a community. Shown with the host's name.
    Hosted,
}

/// One relay in the merged list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub url: Url,
    pub source: Source,
    pub pinned: bool,
}

/// Merges candidate relays into one list of at most `limit` entries.
///
/// `candidates` is in priority order: when a URL appears twice, the earlier source keeps it. Disabled
/// relays are dropped. Pinned relays are kept ahead of the rest when the list is over `limit`. The result
/// keeps the candidates' order.
pub fn merge(candidates: &[(Source, Url)], pinned: &[Url], disabled: &[Url], limit: usize) -> Vec<Entry> {
    let mut unique: Vec<(Source, Url)> = Vec::new();
    for (source, url) in candidates {
        if disabled.contains(url) || unique.iter().any(|(_, u)| u == url) {
            continue;
        }
        unique.push((*source, url.clone()));
    }
    let is_pinned = |url: &Url| pinned.contains(url);
    // Pinned relays fill the list first, in candidate order. Everything else fills whatever room is left.
    let mut keep: Vec<bool> = vec![false; unique.len()];
    let mut room = limit;
    for (i, (_, url)) in unique.iter().enumerate() {
        if room > 0 && is_pinned(url) {
            keep[i] = true;
            room -= 1;
        }
    }
    for (i, (_, url)) in unique.iter().enumerate() {
        if room > 0 && !is_pinned(url) {
            keep[i] = true;
            room -= 1;
        }
    }
    unique
        .into_iter()
        .zip(keep)
        .filter(|(_, kept)| *kept)
        .map(|((source, url), _)| Entry { pinned: is_pinned(&url), url, source })
        .collect()
}

/// Orders relays fastest first, by the best latency measured to each. A relay nothing reached sorts last.
/// The sort is stable, so relays with equal latency keep their order.
pub fn rank(latencies: &[(Url, Option<Duration>)]) -> Vec<(Url, Option<Duration>)> {
    let mut ranked = latencies.to_vec();
    ranked.sort_by_key(|(_, latency)| (latency.is_none(), latency.unwrap_or_default()));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(host: &str) -> Url {
        Url::parse(&format!("https://{host}.example.org/")).unwrap()
    }

    #[test]
    fn earlier_source_keeps_a_duplicate_url() {
        let candidates = [(Source::Custom, url("a")), (Source::Public, url("a")), (Source::Public, url("b"))];
        let merged = merge(&candidates, &[], &[], MAX_LIST);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].source, Source::Custom);
        assert_eq!(merged[1].url, url("b"));
    }

    #[test]
    fn disabled_relays_are_dropped() {
        let candidates = [(Source::Public, url("a")), (Source::Public, url("b"))];
        let merged = merge(&candidates, &[], &[url("a")], MAX_LIST);
        assert_eq!(merged.iter().map(|e| e.url.clone()).collect::<Vec<_>>(), vec![url("b")]);
    }

    #[test]
    fn cap_keeps_the_first_candidates() {
        let candidates: Vec<(Source, Url)> = (0..40).map(|i| (Source::Community, url(&format!("r{i}")))).collect();
        let merged = merge(&candidates, &[], &[], MAX_LIST);
        assert_eq!(merged.len(), MAX_LIST);
        assert_eq!(merged[0].url, url("r0"));
        assert_eq!(merged[MAX_LIST - 1].url, url(&format!("r{}", MAX_LIST - 1)));
    }

    #[test]
    fn pinned_relays_survive_the_cap() {
        let candidates: Vec<(Source, Url)> = (0..40).map(|i| (Source::Community, url(&format!("r{i}")))).collect();
        let pinned = [url("r39")];
        let merged = merge(&candidates, &pinned, &[], MAX_LIST);
        assert_eq!(merged.len(), MAX_LIST);
        let last = merged.iter().find(|e| e.url == url("r39")).expect("pinned relay kept");
        assert!(last.pinned);
        assert!(merged.iter().all(|e| e.url != url("r29") || !e.pinned));
    }

    #[test]
    fn pinned_relays_fill_the_list_before_unpinned_ones() {
        let candidates = [(Source::Public, url("a")), (Source::Public, url("b")), (Source::Public, url("c"))];
        let merged = merge(&candidates, &[url("c")], &[], 2);
        let urls: Vec<Url> = merged.iter().map(|e| e.url.clone()).collect();
        assert_eq!(urls, vec![url("a"), url("c")]);
    }

    #[test]
    fn pins_past_the_limit_are_dropped_in_candidate_order() {
        let candidates = [(Source::Public, url("a")), (Source::Public, url("b")), (Source::Public, url("c"))];
        let merged = merge(&candidates, &[url("a"), url("b"), url("c")], &[], 2);
        let urls: Vec<Url> = merged.iter().map(|e| e.url.clone()).collect();
        assert_eq!(urls, vec![url("a"), url("b")]);
        assert!(merged.iter().all(|e| e.pinned));
    }

    #[test]
    fn a_pinned_relay_that_is_turned_off_is_dropped() {
        let candidates = [(Source::Public, url("a")), (Source::Public, url("b"))];
        let merged = merge(&candidates, &[url("a")], &[url("a")], MAX_LIST);
        assert_eq!(merged.iter().map(|e| e.url.clone()).collect::<Vec<_>>(), vec![url("b")]);
    }

    #[test]
    fn ranks_fastest_first_and_unmeasured_last() {
        let latencies = [
            (url("slow"), Some(Duration::from_millis(180))),
            (url("gone"), None),
            (url("fast"), Some(Duration::from_millis(20))),
        ];
        let ranked: Vec<Url> = rank(&latencies).into_iter().map(|(u, _)| u).collect();
        assert_eq!(ranked, vec![url("fast"), url("slow"), url("gone")]);
    }
}
