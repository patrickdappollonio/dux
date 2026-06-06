import { useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { login, useDux } from "@/lib/store"

// The login screen the SPA renders when auth is ON and there is no session
// (phase === "anonymous"). It is the ONLY thing on screen in that state — no
// sidebar, no shell, no WS-dependent UI — so it can render before any socket
// connects (the connect happens on a successful login). A centered shadcn Card
// holds the dux brand mark, a username/password form, and an error line.
//
// Mobile-sized to the established conventions: a full-width card with page
// margins on small screens (capped to a comfortable width on desktop), 44px
// touch targets, and `text-base` inputs (the shadcn Input default) so iOS does
// not zoom on focus. Enter submits (it is a real <form>, so the submit button's
// click is what the browser fires).
export function LoginScreen() {
  const { auth } = useDux()
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")

  // A submit is dispatched only when both fields are non-empty; the server is
  // the real validator, but gating here avoids a guaranteed-failing round-trip
  // and a misleading "invalid username or password" for an obviously blank form.
  const canSubmit =
    username.trim().length > 0 && password.length > 0 && !auth.pending

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!canSubmit) return
    void login(username.trim(), password)
  }

  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm">
        <CardHeader className="items-center gap-3 text-center">
          <img
            src="/dux-logo.png"
            alt="dux"
            className="size-12 rounded-lg"
          />
          <div className="grid gap-1">
            <CardTitle>Sign in to dux</CardTitle>
            <CardDescription>
              Enter your credentials to continue.
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="grid gap-4">
            <div className="grid gap-2">
              <label htmlFor="login-username" className="text-sm font-medium">
                Username
              </label>
              <Input
                id="login-username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                autoComplete="username"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                disabled={auth.pending}
                className="h-11"
                autoFocus
              />
            </div>
            <div className="grid gap-2">
              <label htmlFor="login-password" className="text-sm font-medium">
                Password
              </label>
              <Input
                id="login-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="current-password"
                disabled={auth.pending}
                className="h-11"
              />
            </div>
            {auth.error ? (
              <p className="text-sm text-destructive" role="alert">
                {auth.error}
              </p>
            ) : null}
            <Button
              type="submit"
              disabled={!canSubmit}
              className="h-11"
            >
              {auth.pending ? "Signing in…" : "Sign in"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
