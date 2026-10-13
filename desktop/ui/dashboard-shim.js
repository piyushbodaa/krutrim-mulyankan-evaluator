"use strict";
/* Feeds the existing At a glance dashboard a saved run. Loaded after preview-fixtures.js,
   which this file replaces. preview/ itself is not edited. */
async function previewAPI(path) {
  const data = window.__KM_DASHBOARD__;
  if (!data) throw new Error("Open results from the Krutrim Mulyankan app.");
  if (path === "/api/tasks" || path === "/api/v1/tasks") return {api_version: "1.0", tasks: data.tasks};
  if (path === "/api/v1/capabilities") return data.capabilities;
  if (path === "/api/v1/jobs") return {api_version: "1.0", jobs: [data.job]};
  if (path === "/api/runs") return {runs: data.runs || []};
  if (path === "/api/configurations") return {configurations: []};
  if (path === "/api/evidence") return {evidence: []};
  if (path === "/api/proposals") return {items: [], status: "excluded_for_release_review"};
  if (path === "/api/pilot" || path === "/api/v1/pilot") return {conditions: []};
  if (path.startsWith("/api/v1/jobs/") && (path.endsWith("/export") || path.endsWith("/results"))) {
    return data.export;
  }
  if (path.startsWith("/api/v1/jobs/")) return data.job;
  if (path.startsWith("/api/runs/")) return data.export;
  throw new Error("This screen only shows the saved run.");
}
