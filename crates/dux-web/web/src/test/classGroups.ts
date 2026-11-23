// Shared by the guards that read class text out of source files. One entry per
// group of strings that end up on one element: a `className="..."` string, a
// `className={...}` expression, or a `cn(` / `clsx(` / `cva(` / `twMerge(` call
// with everything inside it, nested objects and arrays included; any other
// string literal is a group of its own. It reads text, not the program: classes
// set from separate variables or statements, built at runtime, or written in a
// stylesheet are not seen.
const CLASS_CALL = /(?:\bcn|\bclsx|\bcva|\btwMerge|\bclassNames)\s*$/

// The class text of every group in a source, each group joined into one string.
export function classGroupsOf(source: string): string[] {
  const groups: string[][] = []
  // The open brackets: the index of the group each one owns and whether it is a
  // class helper call.
  const stack: { group: number; classCall: boolean }[] = []
  const open = (classCall: boolean) => {
    groups.push([])
    stack.push({ group: groups.length - 1, classCall })
  }
  const add = (text: string, own: boolean) => {
    if (own || stack.length === 0) {
      groups.push([text])
      return
    }
    // Inside a class call the string belongs to that call and to every bracket
    // between it and the string; elsewhere to the innermost bracket alone.
    let top = stack.length - 1
    while (top >= 0 && !stack[top].classCall) top--
    const from = top >= 0 ? top : stack.length - 1
    for (let k = stack.length - 1; k >= from; k--) groups[stack[k].group].push(text)
  }
  let i = 0
  while (i < source.length) {
    const c = source[i]
    if (c === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i++
      continue
    }
    if (c === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2)
      i = end < 0 ? source.length : end + 2
      continue
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1
      let closed = false
      while (j < source.length) {
        if (source[j] === "\\") {
          j += 2
          continue
        }
        if (source[j] === c) {
          closed = true
          break
        }
        // A quote that never closes on its line is an apostrophe in JSX text.
        if (c !== "`" && source[j] === "\n") break
        j++
      }
      if (closed) {
        const before = source.slice(Math.max(0, i - 40), i)
        const own = /[A-Za-z_$][\w$]*\s*=\s*$/.test(before) && !/[=!<>]=\s*$/.test(before)
        add(source.slice(i + 1, j), own)
        i = j + 1
      } else {
        i++
      }
      continue
    }
    if (c === "(" || c === "{" || c === "[") {
      open(c === "(" && CLASS_CALL.test(source.slice(Math.max(0, i - 12), i)))
    } else if (c === ")" || c === "}" || c === "]") {
      stack.pop()
    }
    i++
  }
  return groups.map((g) => g.join(" ").replace(/\s+/g, " ").trim()).filter(Boolean)
}
