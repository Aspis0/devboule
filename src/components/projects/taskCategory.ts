import type { ProjectTaskCategory } from "../../types/backend";

// Card category metadata for the Board: the four allowed categories
// (mandatory on create in the Todo column) and their on-brand chip styling.
// Kept as a pure module so both the add-card form and the TaskCard chip share
// one source of truth and it stays unit-testable without a DOM.

export const TASK_CATEGORIES: readonly ProjectTaskCategory[] = [
  "feature",
  "hardening",
  "bug",
  "other",
] as const;

interface CategoryMeta {
  label: string;
  // Tailwind classes for the small chip, reusing the existing design tokens
  // (cream / teal / terracotta / sage palette) so categories read as part of
  // the same calm card language, not a new look.
  chipClass: string;
}

const CATEGORY_META: Record<ProjectTaskCategory, CategoryMeta> = {
  feature: {
    label: "Feature",
    chipClass: "bg-teal/10 text-teal",
  },
  hardening: {
    label: "Hardening",
    chipClass: "bg-sage/15 text-sage",
  },
  bug: {
    label: "Bug",
    chipClass: "bg-coral/10 text-coral",
  },
  other: {
    label: "Other",
    chipClass: "bg-cream-200 text-cream-600",
  },
};

export function isTaskCategory(value: unknown): value is ProjectTaskCategory {
  return (
    typeof value === "string" &&
    (TASK_CATEGORIES as readonly string[]).includes(value)
  );
}

export function categoryLabel(category: ProjectTaskCategory): string {
  return CATEGORY_META[category].label;
}

export function categoryChipClass(category: ProjectTaskCategory): string {
  return CATEGORY_META[category].chipClass;
}
