//! "Did you mean" hints for unknown names in a format error.

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1])));
        }
        prev = cur;
    }
    prev[b.len()]
}

pub(crate) fn suggest(name: &str, pool: &[&[&str]]) -> String {
    pool.iter()
        .flat_map(|p| p.iter())
        .map(|c| (edit_distance(name, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map(|(_, c)| format!(" (did you mean `{c}`?)"))
        .unwrap_or_default()
}
