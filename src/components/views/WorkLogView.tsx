/**
 * Chronological record of the user's work.
 *
 * Entries are read from the local SQLite `work_logs` table once the database
 * layer lands in Step 2.
 */
export function WorkLogView() {
  return (
    <div className="mx-auto max-w-3xl p-6">
      <div className="rounded-lg border border-dashed border-border p-10 text-center">
        <h2 className="text-sm font-semibold">No entries yet</h2>
        <p className="mx-auto mt-2 max-w-sm text-sm text-muted-foreground">
          Your daily log will fill in from your connected tools once the local database and
          background summariser are in place.
        </p>
      </div>
    </div>
  );
}
