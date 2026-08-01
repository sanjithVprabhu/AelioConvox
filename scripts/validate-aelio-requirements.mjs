import { access, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const indexPath = path.join(root, "docs/requirements/aelio.json");
const document = JSON.parse(await readFile(indexPath, "utf8"));
const allowed = new Set(document.allowed_statuses ?? []);
const ids = new Set();
const failures = [];

for (const requirement of document.requirements ?? []) {
  const label = requirement.id ?? "<missing-id>";
  if (!/^[A-Z][A-Z0-9-]+-[0-9]{3}$/.test(label)) {
    failures.push(`${label}: id must match CATEGORY-NAME-001`);
  }
  if (ids.has(label)) failures.push(`${label}: duplicate id`);
  ids.add(label);

  for (const field of ["source", "summary", "owner", "status"]) {
    if (typeof requirement[field] !== "string" || requirement[field].trim() === "") {
      failures.push(`${label}: missing ${field}`);
    }
  }
  if (!allowed.has(requirement.status)) {
    failures.push(`${label}: unknown status ${JSON.stringify(requirement.status)}`);
  }
  for (const field of ["code", "tests"]) {
    if (!Array.isArray(requirement[field])) {
      failures.push(`${label}: ${field} must be an array`);
      continue;
    }
    for (const relative of requirement[field]) {
      if (path.isAbsolute(relative) || relative.includes("..")) {
        failures.push(`${label}: unsafe ${field} path ${relative}`);
        continue;
      }
      try {
        await access(path.join(root, relative));
      } catch {
        failures.push(`${label}: missing ${field} path ${relative}`);
      }
    }
  }
  if (requirement.status === "implemented") {
    if (requirement.code.length === 0) failures.push(`${label}: implemented without code evidence`);
    if (requirement.tests.length === 0) failures.push(`${label}: implemented without test evidence`);
  }
  if (requirement.status === "missing" && requirement.code.length + requirement.tests.length > 0) {
    failures.push(`${label}: missing requirement must not imply implementation evidence`);
  }
}

if (failures.length > 0) {
  process.stderr.write(`${failures.join("\n")}\n`);
  process.exitCode = 1;
} else {
  const counts = Object.fromEntries(
    [...allowed].map((status) => [
      status,
      document.requirements.filter((requirement) => requirement.status === status).length,
    ]),
  );
  process.stdout.write(`Aelio requirements valid: ${document.requirements.length} ${JSON.stringify(counts)}\n`);
}
