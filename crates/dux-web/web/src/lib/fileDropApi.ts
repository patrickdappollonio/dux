// Uploading one dropped file. One file per request, raw body, filename as a
// query parameter.
//
// The route saves the file and returns where it landed; it never writes to the
// terminal, because writing is gated on holding input on the websocket and an
// upload handler injecting the path would walk straight past that gate. The
// caller pastes the returned path over its own already-gated socket.

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

/// `dir` switches the route between dux's two drop intents by presence, not
/// emptiness: `undefined` is a drop on a pane, while any string, the empty one
/// included, is a drop on the editor's file tree naming a worktree-relative
/// folder, where "" is the worktree root. The terminal socket's connection id
/// travels in `conn` rather than the `x-connection-id` header, which names the
/// events socket and which the server refuses a PTY-class id in.
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
