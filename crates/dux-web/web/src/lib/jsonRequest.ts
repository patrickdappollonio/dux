import { apiFetch, rethrowAuthInterruption } from "./apiFetch"
import { AttachedError, attachedBlockers } from "./attached"
import { getConnectionId } from "./connection"

type RequestErrorFactory = (
  message: string,
  status: number,
  responseText: string,
) => Error

interface JsonRequestOptions {
  mapSerializationErrors?: boolean
}

function serializeJsonBody(body: unknown): string | undefined {
  return body === undefined ? undefined : JSON.stringify(body)
}

async function responseError(
  response: Response,
  createError: RequestErrorFactory,
): Promise<Error> {
  const detail = (await response.text().catch(() => "")).trim()
  // A refusal because somebody else is attached is the same answer from every
  // guarded route, so it is read here once, as itself, rather than handed on as
  // a raw JSON message for each client to toast.
  const blockers = attachedBlockers(response.status, parseJson(detail))
  if (blockers !== null) return new AttachedError(blockers)
  const message = detail || `request failed (${response.status})`
  return createError(message, response.status, detail)
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

async function parseJsonResponse<T>(response: Response): Promise<T> {
  if (response.status === 204) return undefined as T

  const text = await response.text().catch(() => "")
  if (!text) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    return undefined as T
  }
}

export function createJsonRequest(
  createError: RequestErrorFactory,
  options: JsonRequestOptions = {},
) {
  const mapSerializationErrors = options.mapSerializationErrors ?? false

  return async function request<T>(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<T> {
    const headers: Record<string, string> = {}
    const id = getConnectionId()
    if (id) headers["x-connection-id"] = id

    if (body !== undefined) {
      headers["content-type"] = "application/json"
    }

    let payload: string | undefined
    if (!mapSerializationErrors) payload = serializeJsonBody(body)

    let response: Response
    try {
      if (mapSerializationErrors) payload = serializeJsonBody(body)
      response = await apiFetch(path, {
        method,
        credentials: "same-origin",
        headers,
        body: payload,
      })
    } catch (e) {
      rethrowAuthInterruption(e)
      throw createError("Could not reach the server.", 0, "")
    }

    if (!response.ok) throw await responseError(response, createError)
    return parseJsonResponse<T>(response)
  }
}
