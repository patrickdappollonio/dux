// Uploading one dropped file. One file per request, raw body, filename as a
// query parameter.
//
// The route SAVES the file and returns where it landed. It never writes to the
// terminal: that is gated on holding input, enforced on the websocket, and an
// upload handler injecting the path would walk straight past the gate. The
// caller pastes the returned path over its own already-gated socket.

/// The TERMINAL SOCKET's connection id travels in `conn`, deliberately NOT in
/// the `x-connection-id` header the other API modules stamp. That header names
/// the EVENTS socket, and the server explicitly refuses a PTY-class id in it, so
/// sending it there would be checking a different thing entirely.
export interface SavedDropResponse {
  path: string
  saved_name: string
  requested_name: string
  folder: string
  folder_label: string
  renamed: boolean
}

export class FileDropApiError extends Error {
  readonly status: number
  constructor(message: string, status: number) {
    super(message)
    this.name = "FileDropApiError"
    this.status = status
  }
}

/// `dir` is what switches the route between dux's two drop intents, and it is
/// distinguished by PRESENCE and not by emptiness: `undefined` is a drop on a
/// pane (the file goes to the agent's invisible upload folder, or to where the
/// terminal actually is), while any string, INCLUDING the empty one, is a drop
/// on the editor's file tree and names the worktree-relative folder the user
/// dropped on. The empty string is the worktree root, which is a perfectly
/// ordinary place to drop, so it must not be treated as "no directory".
export async function uploadDroppedFile(
  file: File,
  opts: { pty: string; conn: string | null; dir?: string },
): Promise<SavedDropResponse> {
  const params = new URLSearchParams({ pty: opts.pty, filename: file.name })
  if (opts.conn) params.set("conn", opts.conn)
  if (opts.dir !== undefined) params.set("dir", opts.dir)
  let resp: Response
  try {
    resp = await fetch(`/api/v1/file-drop?${params.toString()}`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/octet-stream" },
      body: file,
    })
  } catch {
    throw new FileDropApiError("could not reach the server", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new FileDropApiError(
      detail || `the server refused the upload (${resp.status})`,
      resp.status,
    )
  }
  return (await resp.json()) as SavedDropResponse
}
