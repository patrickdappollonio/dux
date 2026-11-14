// What a project-settings save says when it lands, or when there was nothing to
// save. The engine raises no status for these changes, so the browser confirms
// them itself: the dialog closing is not proof that anything was written.

import type { PatchProjectBody } from "./projectsApi"
import { chip, type Prose, prose } from "./prose"

/// Which part of a project's settings a dialog edits, named in the
/// nothing-to-save sentence.
export type ProjectSettingsSubject = "settings" | "environment" | "startup command"

const FIELD_WORDS: Record<keyof PatchProjectBody, string> = {
  provider: "default provider",
  auto_reopen_agents: "auto-reopen setting",
  startup_command: "startup command",
  env: "environment",
}

// "a", "a and b", "a, b and c".
function listWords(words: string[]): string {
  if (words.length <= 1) return words.join("")
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`
}

/// "Saved the default provider and environment for project <name>."
export function projectSettingsSaved(projectName: string, patch: PatchProjectBody): Prose {
  const fields = (Object.keys(FIELD_WORDS) as (keyof PatchProjectBody)[])
    .filter((key) => key in patch)
    .map((key) => FIELD_WORDS[key])
  return prose`Saved the ${listWords(fields)} for project ${chip(projectName)}.`
}

/// "Nothing to save: the settings for project <name> are unchanged."
export function projectSettingsUnchanged(
  projectName: string,
  subject: ProjectSettingsSubject,
): Prose {
  const verb = subject === "settings" ? "are" : "is"
  return prose`Nothing to save: the ${subject} for project ${chip(projectName)} ${verb} unchanged.`
}
