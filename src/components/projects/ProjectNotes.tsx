import { CollapsibleSection } from "./CollapsibleSection";
import { FileText } from "lucide-react";
import { formatDate } from "./projectFormat";
import type { ProjectNote } from "../../types/backend";

export interface ProjectNotesProps {
  notes: ProjectNote[];
  noteDraft: string;
  onNoteDraftChange: (value: string) => void;
  onAppend: () => void;
  isBusy: boolean;
  revision: string;
  modifiedAt: string;
  updatedAt: string;
}

export function ProjectNotes({
  notes,
  noteDraft,
  onNoteDraftChange,
  onAppend,
  isBusy,
  revision,
  modifiedAt,
  updatedAt,
}: ProjectNotesProps) {
  return (
    <CollapsibleSection
      icon={FileText}
      title="Notes"
      purpose="The project's running log — what agents did, plus reminders"
      summary={`${notes.length} notes`}
      helpTitle="Notes keep project memory and decisions."
      helpLines="Notes are written into the project Markdown file.|Use notes for decisions, evidence, smoke results, and blocked reasons.|Oracle can retrieve notes after indexing catches the change.|Do not paste raw secrets or private tokens into notes."
    >
      <p className="mb-3 text-[12px] text-cream-500">
        Every important action or reminder an agent leaves lands here —
        it's what a verifier reads before marking work Done.
      </p>
      <div className="flex gap-2">
        <textarea
          value={noteDraft}
          onChange={(event) => onNoteDraftChange(event.target.value)}
          placeholder="Append a project note"
          rows={3}
          data-help-title="A note is durable project memory."
          data-help-lines="Notes are written into the project Markdown file.|Use notes for decisions, evidence, smoke results, and blocked reasons.|Oracle can retrieve notes after indexing catches the change.|Do not paste raw secrets or private tokens into notes."
          className="min-w-0 flex-1 resize-none rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
        />
        <button
          onClick={() => void onAppend()}
          disabled={isBusy || !noteDraft.trim()}
          data-help-title="This appends the note to the project file."
          data-help-lines="Appending is a local Markdown write.|It is useful for human decisions and agent evidence.|The note becomes searchable by Oracle after incremental indexing.|Do not use it for secret values or temporary API keys."
          className="self-start rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
        >
          Append
        </button>
      </div>
      <div className="mt-4 space-y-2">
        {notes.length === 0 ? (
          <p className="text-[12px] text-cream-400">No notes yet.</p>
        ) : (
          [...notes]
            .reverse()
            .slice(0, 8)
            .map((note) => (
              <div key={note.id} className="rounded-lg bg-cream-50 px-3 py-2">
                <p className="break-words text-[12px] leading-5 text-cream-700">
                  {note.text}
                </p>
                <p className="mt-1 text-[10px] text-cream-400">
                  {note.source} / {formatDate(note.createdAt)}
                </p>
              </div>
            ))
        )}
      </div>
      <div className="mt-4 border-t border-cream-200 pt-3">
        <div className="mb-2 flex items-center gap-2">
          <FileText className="h-4 w-4 text-teal" />
          <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Sync
          </h3>
        </div>
        <div className="space-y-2 text-[12px] text-cream-600">
          <p>
            Revision:{" "}
            <span className="font-mono text-[10px]">
              {revision.slice(0, 12)}
            </span>
          </p>
          <p>Modified: {formatDate(modifiedAt)}</p>
          <p>Updated: {formatDate(updatedAt)}</p>
        </div>
      </div>
    </CollapsibleSection>
  );
}
