// An open menu as the order a person reads it, for tests that pin where a
// reason sits: one entry per child of the popup, in the rendered order, so a
// test says what is next to what rather than which classes something carries.
//
// - an item or a submenu trigger is its text;
// - a reason line is `reason: ` and its sentence;
// - a separator is `---`;
// - anything else (a group with a heading, say) is its text, rows and all.

/** The popup's children, as described above. */
export function menuLayout(menu: HTMLElement): string[] {
  return Array.from(menu.children).map((el) => {
    if (el.getAttribute("role") === "separator") return "---"
    if (el.getAttribute("data-slot") === "menu-reason-group") {
      return `reason: ${el.textContent ?? ""}`
    }
    return (el.textContent ?? "").trim()
  })
}

/** What is wrong with the menu's rules, or an empty list: none at either end, none twice running. */
export function separatorProblems(layout: readonly string[]): string[] {
  const problems: string[] = []
  if (layout[0] === "---") problems.push("starts on a separator")
  if (layout[layout.length - 1] === "---") problems.push("ends on a separator")
  layout.forEach((entry, i) => {
    if (entry === "---" && layout[i + 1] === "---") problems.push(`two separators at ${i}`)
  })
  return problems
}
