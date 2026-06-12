// Shared Board-Kanban column types used by both ProjectsView (which owns the
// task move handler + gating) and the extracted TaskCard / MiniMenu UI. Kept in
// its own module so the presentational card can type its move targets without
// importing back into the view (avoiding a circular import).

export type ColumnId = "todo" | "wip" | "review" | "blocked" | "done";

export interface MoveTarget {
  id: ColumnId;
  label: string;
}
