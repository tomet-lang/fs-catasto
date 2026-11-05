use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

pub fn check(path: &Path, all: bool, json: bool) -> Result<ExitCode> {
    if all {
        check_all(path, json)
    } else {
        check_single(path, json)
    }
}

fn check_single(path: &Path, json: bool) -> Result<ExitCode> {
    let report = catasto_core::check(path)?;
    if json {
        let obj = serde_json::json!({
            "root": path,
            "ok": report.is_ok(),
            "violations": report.violations.iter().map(|v| {
                serde_json::json!({
                    "path": v.path,
                    "kind": v.kind.label(),
                    "message": v.kind.to_string(),
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        if report.is_ok() {
            Ok(ExitCode::SUCCESS)
        } else {
            Ok(ExitCode::FAILURE)
        }
    } else if report.is_ok() {
        Ok(ExitCode::SUCCESS)
    } else {
        for violation in &report.violations {
            eprintln!("catasto: {violation}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn check_all(root: &Path, json: bool) -> Result<ExitCode> {
    let workspaces = catasto_core::discover_workspaces(root)?;
    if workspaces.is_empty() {
        if json {
            let obj = serde_json::json!({
                "workspaces": [],
                "summary": {
                    "total": 0,
                    "passed": 0,
                    "failed": 0,
                }
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("catasto: no workspaces discovered under {}", root.display());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let total = workspaces.len();
    let mut passed = 0;
    let mut failed = 0;
    let mut json_results = Vec::new();

    for (idx, ws) in workspaces.iter().enumerate() {
        let num = idx + 1;
        let ws_display = ws.display();
        match catasto_core::check(ws) {
            Ok(report) => {
                let is_ok = report.is_ok();
                if is_ok {
                    passed += 1;
                    if !json {
                        println!("[{num}/{total}] {ws_display} ... ok");
                    }
                } else {
                    failed += 1;
                    if !json {
                        let count = report.violations.len();
                        let word = if count == 1 { "violation" } else { "violations" };
                        println!("[{num}/{total}] {ws_display} ... failed ({count} {word})");
                        for v in &report.violations {
                            println!("      catasto: {v}");
                        }
                    }
                }
                if json {
                    json_results.push(serde_json::json!({
                        "root": ws,
                        "ok": is_ok,
                        "violations": report.violations.iter().map(|v| {
                            serde_json::json!({
                                "path": v.path,
                                "kind": v.kind.label(),
                                "message": v.kind.to_string(),
                            })
                        }).collect::<Vec<_>>(),
                    }));
                }
            }
            Err(e) => {
                failed += 1;
                if !json {
                    println!("[{num}/{total}] {ws_display} ... error: {e:#}");
                }
                if json {
                    json_results.push(serde_json::json!({
                        "root": ws,
                        "ok": false,
                        "error": format!("{e:#}"),
                        "violations": [],
                    }));
                }
            }
        }
    }

    if json {
        let obj = serde_json::json!({
            "workspaces": json_results,
            "summary": {
                "total": total,
                "passed": passed,
                "failed": failed,
            }
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("Summary: {passed} passed, {failed} failed ({total} total)");
    }

    if failed > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
