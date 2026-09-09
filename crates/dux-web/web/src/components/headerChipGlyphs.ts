import { Bot, Cpu, Folder, GitBranch, SquareTerminal } from "lucide-react"
import type { LucideIcon } from "lucide-react"

import type { HeaderChipKind } from "@/lib/headerSubject"

// The glyph per chip kind, shared by the desktop header row (`InsetHeader`) and
// the phone header's two lanes (`MobileShell`) so one chip kind can never be
// drawn as two different things. It lives beside the components rather than in
// `lib/headerSubject.ts` because it is the only part of the chip model that
// needs a React icon; the model itself stays free of any component import.
//
// Each glyph is the one dux already draws for that thing elsewhere, so the
// header and the rest of the app teach each other rather than inventing a
// second vocabulary. The robot deliberately does not double as the assistant:
// it already means "an agent", and reusing it would say the agent's name and
// the model behind it are the same kind of thing. `directory` shares the folder
// because a directory is a folder, and the two never appear in one row.
export const CHIP_GLYPHS: Record<HeaderChipKind, LucideIcon> = {
  project: Folder,
  agent: Bot,
  branch: GitBranch,
  terminal: SquareTerminal,
  assistant: Cpu,
  directory: Folder,
}
