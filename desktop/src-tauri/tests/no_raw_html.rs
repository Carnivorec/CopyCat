use std::path::Path;

const FORBIDDEN: [&str; 5] =
    ["dangerouslySetInnerHTML", "innerHTML", "outerHTML", "insertAdjacentHTML", "document.write"];

fn scan(dir: &Path, hits: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            scan(&p, hits);
        } else if p.extension().is_some_and(|x| x == "ts" || x == "tsx") {
            let text = std::fs::read_to_string(&p).unwrap();
            for (n, line) in text.lines().enumerate() {
                for f in FORBIDDEN {
                    if line.contains(f) {
                        hits.push(format!("{}:{}: {f}", p.display(), n + 1));
                    }
                }
            }
        }
    }
}

#[test]
fn ui_never_injects_html_strings() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
    let mut hits = Vec::new();
    scan(&src, &mut hits);
    assert!(hits.is_empty(), "界面代码中出现了插入 HTML 字符串的写法：\n{}", hits.join("\n"));
}
