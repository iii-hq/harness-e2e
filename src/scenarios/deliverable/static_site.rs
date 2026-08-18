//! A two-page static site. Graded on structure a reviewer would check by
//! hand: the file set, one heading per page, described images, resolvable
//! links, and no calls out to another host.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.static_site";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "static_site_artifact";
const FILES: [&str; 4] = [
    "site/about.html",
    "site/index.html",
    "site/routes.json",
    "site/style.css",
];
const PAGES: [(&str, &str); 2] = [("/", "index.html"), ("/about", "about.html")];

const FILE_SET_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "file_set_exact",
    25,
    "The site directory holds exactly the four requested files.",
);
const PAGE_STRUCTURE: AssessmentSpec = AssessmentSpec::hard_gated(
    "page_structure",
    30,
    "Each page has one top-level heading, every image is described, and every link resolves.",
);
const SELF_CONTAINED: AssessmentSpec = AssessmentSpec::hard_gated(
    "self_contained",
    20,
    "No page or stylesheet references another host.",
);
const ROUTES_DECLARED: AssessmentSpec = AssessmentSpec::hard_gated(
    "routes_declared",
    25,
    "The route table maps exactly the declared paths to the pages that exist.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    FILE_SET_EXACT,
    PAGE_STRUCTURE,
    SELF_CONTAINED,
    ROUTES_DECLARED,
];

fn expected_routes() -> Vec<Value> {
    PAGES
        .iter()
        .map(|(path, file)| json!({ "path": path, "file": file }))
        .collect()
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: String::from(
            "Build a two-page static site in this workspace, under `site/`.\n\n\
             1. Write exactly four files and nothing else: `site/index.html`, \
             `site/about.html`, `site/style.css`, and `site/routes.json`.\n\
             2. Each page must have exactly one `<h1>`, link to the other page with a relative \
             href, and load `style.css` with a relative href. Every `<img>` you include needs a \
             non-empty `alt`.\n\
             3. The site must work offline: no `http://`, no `https://`, no CDN, no remote \
             fonts.\n\
             4. `site/routes.json` is a JSON array of {\"path\": ..., \"file\": ...} objects \
             mapping `/` to `index.html` and `/about` to `about.html`.\n\
             5. Reply with exactly one line: `PAGES:2 FILES:4`.",
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(22, 300_000, 420),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "files": FILES,
            "routes": expected_routes(),
        }),
        super::build_profile(4, 3),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["files", "routes", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

struct SiteEvidence {
    files: Vec<String>,
    routes: Vec<Value>,
    headings_correct: bool,
    images_described: bool,
    links_resolve: bool,
    external: Vec<String>,
}

fn inspect(run_id: &str) -> SiteEvidence {
    let root = workspace::root(ID, run_id);
    let files = workspace::files_under(&root, "site");
    let mut headings_correct = true;
    let mut images_described = true;
    let mut links_resolve = true;
    let mut external = Vec::new();
    for (_, page) in PAGES {
        let relative = format!("site/{page}");
        let Some(html) = workspace::read(&root, &relative) else {
            headings_correct = false;
            links_resolve = false;
            continue;
        };
        headings_correct &= workspace::count_elements(&html, "h1") == 1;
        images_described &= workspace::images_without_alt(&html) == 0;
        for link in workspace::local_links(&html) {
            let target = link.split(['#', '?']).next().unwrap_or_default();
            links_resolve &= !target.is_empty() && root.join("site").join(target).exists();
        }
        external.extend(workspace::external_references(&html));
    }
    if let Some(stylesheet) = workspace::read(&root, "site/style.css") {
        external.extend(workspace::external_references(&stylesheet));
    }
    external.sort();
    external.dedup();
    let routes = workspace::read_json(&root, "site/routes.json")
        .and_then(|routes| routes.as_array().cloned())
        .unwrap_or_default();
    SiteEvidence {
        files,
        routes,
        headings_correct,
        images_described,
        links_resolve,
        external,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let evidence = inspect(run_id);
        let expected_files: Vec<String> = FILES.iter().map(|file| (*file).to_string()).collect();

        Ok(assessment::build_evaluation([
            FILE_SET_EXACT.full_or_zero(
                evidence.files == expected_files,
                format!("observed files {:?}", evidence.files),
            ),
            PAGE_STRUCTURE.full_or_zero(
                evidence.headings_correct && evidence.images_described && evidence.links_resolve,
                format!(
                    "single heading per page: {}; every image described: {}; every link resolves: {}",
                    evidence.headings_correct, evidence.images_described, evidence.links_resolve
                ),
            ),
            SELF_CONTAINED.full_or_zero(
                evidence.external.is_empty(),
                format!("external reference(s): {:?}", evidence.external),
            ),
            ROUTES_DECLARED.full_or_zero(
                kit::sorted_by(&evidence.routes, "path") == kit::sorted_by(&expected_routes(), "path")
                    && observation.response.contains("PAGES:2 FILES:4"),
                format!("observed routes {:?}", evidence.routes),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let evidence = inspect(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "files": evidence.files,
                "routes": evidence.routes,
                "external_references": evidence.external,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_static_site_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}
