// One labelled row in an info-dialog definition list. The value column is
// allowed to wrap (paths, branch names) so long values stay readable on phones.
// Shared by `ProjectInfoDialog` and `AgentInfoDialog` so the two info modals stay
// visually identical.
export function InfoRow({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1 max-sm:grid-cols-1">
      <dt className="text-sm text-muted-foreground">{label}</dt>
      <dd className="min-w-0 text-sm">{children}</dd>
    </div>
  )
}
