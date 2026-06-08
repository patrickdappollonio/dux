// A constant-length light that travels around a working agent row's outline. The
// styling (geometry, stroke, dash, animation) lives in `.agent-beam` in
// index.css; `pathLength={100}` is what keeps the dash a fixed fraction of the
// perimeter, so the segment never grows or shrinks as it rounds the corners.
// Render inside a `relative` row, only while the agent is working.
export function AgentBeam() {
  return (
    <svg aria-hidden className="agent-beam">
      <rect x="0" y="0" width="100%" height="100%" rx="6" pathLength={100} />
    </svg>
  )
}
