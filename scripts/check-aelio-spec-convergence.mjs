import { access, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const motherPath = "docs/claude_context/AELIO_DSL_MOTHER.md";
const technicalPath = "docs/claude_context/TECHNICAL_MOTHER_SPECIFICATION.md";
const planPath = "docs/claude_context/AELIO_UNIFIED_IMPLEMENTATION_MASTER_PLAN.md";
const [mother, technical, plan] = await Promise.all(
  [motherPath, technicalPath, planPath].map((relative) =>
    readFile(path.join(root, relative), "utf8"),
  ),
);
const failures = [];

const forbidden = [
  ["obsolete database name", /Sunjet/],
  ["second canonical Unicode form", /strings NFC-normal(?:ised|ized)/i],
  ["obsolete five-input promotion threshold", /K\s*=\s*20\s*[,/]\s*M\s*=\s*5/],
  ["re-executed historical recall", /recall[^\n]{0,40}MAY re-execute/i],
  ["stale unresolved conversion attack", /- \[ \] A14\./],
  ["obsolete @v pin grammar", /@v(?:N|[0-9]+)/],
];
for (const [name, pattern] of forbidden) {
  for (const [relative, text] of [[motherPath, mother], [technicalPath, technical]]) {
    if (pattern.test(text)) failures.push(`${relative}: ${name}`);
  }
}

const required = [
  [motherPath, mother, "AMENDMENT #4"],
  [motherPath, mother, "aelio-db.prism@1"],
  [motherPath, mother, "read_result"],
  [technicalPath, technical, "**Version:** 1.6"],
  [technicalPath, technical, "D-41"],
  [technicalPath, technical, "no Unicode normalization"],
  [technicalPath, technical, "injected from the record"],
  [planPath, plan, "## 13. Definition of fully integrated"],
];
for (const [relative, text, fragment] of required) {
  if (!text.includes(fragment)) failures.push(`${relative}: missing ${JSON.stringify(fragment)}`);
}

for (const [relative, text] of [[motherPath, mother], [technicalPath, technical], [planPath, plan]]) {
  const fences = text.split("\n").filter((line) => line.startsWith("```")).length;
  if (fences % 2 !== 0) failures.push(`${relative}: unbalanced Markdown fences (${fences})`);
}

const annexes = [2, 3, 4, 5, 6, 10, 11, 14].map((number) => {
  const names = {
    2: "F2_path_grammar.md",
    3: "F3_compute_signatures.md",
    4: "F4_conversion_rules.md",
    5: "F5_sense_v1.md",
    6: "F6_registry_entries.md",
    10: "F10_aelio_db_ddl.md",
    11: "F11_conformance_vectors.md",
    14: "F14_metrics_schema.md",
  };
  return `docs/annexes/${names[number]}`;
});
for (const relative of annexes) {
  try {
    await access(path.join(root, relative));
  } catch {
    failures.push(`missing completed annex ${relative}`);
  }
}

if (failures.length > 0) {
  process.stderr.write(`${failures.join("\n")}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write("Aelio normative specifications converge on Prism, Sol, replay, paths, and lifecycle.\n");
}
