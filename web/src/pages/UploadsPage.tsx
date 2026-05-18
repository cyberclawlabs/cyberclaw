// UploadsPage — drag-and-drop file uploads with per-file status.
//
// Wraps /api/v1/uploads (POST multipart / GET list / DELETE :id).
// Server enforces 25 MB cap, filename sanitize, magic-byte mime sniff,
// path-traversal guard, and emits upload.create / upload.delete audit
// rows. Frontend just provides the UX.

import { useEffect, useState, useCallback } from "react";
import { type Lang } from "@/lib/i18n";
import Modal from "@/components/Modal";
import {
  fetchUploads,
  uploadFile,
  deleteUpload,
  type UploadListEntry,
} from "@/lib/api";

function fmtBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(2)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${n} B`;
}

function fmtTime(s?: string): string {
  if (!s) return "—";
  try {
    return new Date(s).toLocaleString();
  } catch {
    return s;
  }
}

interface PendingUpload {
  file: File;
  status: "queued" | "uploading" | "done" | "error";
  error?: string;
}

export default function UploadsPage({ lang }: { lang: Lang }) {
  const zh = lang === "zh-CN";
  const t = zh
    ? {
        title: "文件上传",
        subtitle: "上传到工作区 /uploads/。受限 25 MB / 文件，路径自动消毒。",
        dropZone: "拖拽文件到这里或点击选择",
        loading: "加载中…",
        emptyTitle: "暂无文件",
        emptyHint: "拖拽文件到上方区域上传。所有上传会写入审计链。",
        colName: "文件名",
        colSize: "大小",
        colTime: "上传时间",
        colAction: "操作",
        deleteBtn: "删除",
        deleting: "删除中…",
        confirmDelete: "确认删除该文件？",
        confirmDeleteTitle: "删除上传文件",
        confirmDeleteBtn: "删除",
        confirmCancelBtn: "取消",
        statusQueued: "等待",
        statusUploading: "上传中",
        statusDone: "完成",
        statusError: "错误",
      }
    : {
        title: "Uploads",
        subtitle:
          "Files land in <workspace>/uploads/. 25 MB max per file, filename auto-sanitized.",
        dropZone: "Drop files here or click to choose",
        loading: "Loading…",
        emptyTitle: "No uploads yet",
        emptyHint:
          "Drag a file onto the zone above. Every upload is recorded in the audit chain.",
        colName: "Filename",
        colSize: "Size",
        colTime: "Uploaded at",
        colAction: "Actions",
        deleteBtn: "Delete",
        deleting: "Deleting…",
        confirmDelete: "Delete this upload?",
        confirmDeleteTitle: "Delete upload",
        confirmDeleteBtn: "Delete",
        confirmCancelBtn: "Cancel",
        statusQueued: "Queued",
        statusUploading: "Uploading",
        statusDone: "Done",
        statusError: "Error",
      };

  const [uploads, setUploads] = useState<UploadListEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingUpload[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await fetchUploads();
      setUploads(list);
      setError(null);
    } catch (e) {
      console.error("UploadsPage: fetch failed", e);
      setError(e instanceof Error ? e.message : String(e));
      setUploads([]);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleFiles = async (files: FileList | File[]) => {
    const arr = Array.from(files);
    setPending((prev) => [
      ...prev,
      ...arr.map((f) => ({ file: f, status: "queued" as const })),
    ]);
    for (const f of arr) {
      setPending((prev) =>
        prev.map((p) =>
          p.file === f ? { ...p, status: "uploading" } : p,
        ),
      );
      try {
        await uploadFile(f);
        setPending((prev) =>
          prev.map((p) =>
            p.file === f ? { ...p, status: "done" } : p,
          ),
        );
      } catch (e) {
        setPending((prev) =>
          prev.map((p) =>
            p.file === f
              ? {
                  ...p,
                  status: "error",
                  error: e instanceof Error ? e.message : String(e),
                }
              : p,
          ),
        );
      }
    }
    await refresh();
    // Auto-clear done entries after a moment so the panel doesn't grow forever.
    setTimeout(() => {
      setPending((prev) => prev.filter((p) => p.status !== "done"));
    }, 3000);
  };

  // window.confirm replaced with a themed Modal — same fix as ChatPage
  // (P1-J pt2). Native confirm is unstyled and is silently no-op'd by some
  // PWA / Electron / browser-policy contexts.
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const onDelete = (id: string) => {
    setPendingDelete(id);
  };
  const confirmDelete = async () => {
    const id = pendingDelete;
    if (!id) return;
    setPendingDelete(null);
    setDeleting(id);
    try {
      await deleteUpload(id);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDeleting(null);
    }
  };

  return (
    <div className="space-y-4 max-w-5xl">
      <header className="space-y-1">
        <h1 className="text-lg font-semibold">{t.title}</h1>
        <p className="text-[12px] text-fg-3">{t.subtitle}</p>
      </header>

      {/* Drop zone */}
      <label
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          if (e.dataTransfer.files.length > 0) {
            handleFiles(e.dataTransfer.files);
          }
        }}
        className={
          "block rounded-lg border-2 border-dashed p-8 text-center cursor-pointer transition-colors " +
          (dragOver
            ? "border-accent bg-accent-soft text-accent"
            : "border-border bg-bg-2 text-fg-3 hover:border-accent/60 hover:text-fg-2")
        }
      >
        <input
          type="file"
          multiple
          className="hidden"
          onChange={(e) => {
            if (e.target.files && e.target.files.length > 0) {
              handleFiles(e.target.files);
              e.target.value = "";
            }
          }}
        />
        <div className="text-[13px]">{t.dropZone}</div>
      </label>

      {/* Pending status */}
      {pending.length > 0 && (
        <ul className="space-y-1 text-[12px]">
          {pending.map((p, i) => (
            <li key={i} className="flex items-center justify-between gap-3 px-3 py-1.5 rounded border border-border bg-bg-2">
              <span className="truncate flex-1">{p.file.name} ({fmtBytes(p.file.size)})</span>
              <span
                className={
                  "shrink-0 text-[11px] mono " +
                  (p.status === "done"
                    ? "text-green-400"
                    : p.status === "error"
                      ? "text-red-400"
                      : "text-fg-3")
                }
              >
                {p.status === "queued"
                  ? t.statusQueued
                  : p.status === "uploading"
                    ? t.statusUploading
                    : p.status === "done"
                      ? t.statusDone
                      : `${t.statusError}: ${p.error}`}
              </span>
            </li>
          ))}
        </ul>
      )}

      {/* List */}
      {uploads === null && <p className="text-[12px] text-fg-3">{t.loading}</p>}
      {uploads !== null && uploads.length === 0 && (
        <div className="rounded-lg border border-border bg-bg-2 p-6 text-center space-y-1">
          <div className="text-[13px] font-medium">{t.emptyTitle}</div>
          <div className="text-[11px] text-fg-3">{t.emptyHint}</div>
        </div>
      )}
      {uploads !== null && uploads.length > 0 && (
        <table className="w-full text-[12px] border border-border rounded-md overflow-hidden">
          <thead className="bg-bg-3">
            <tr className="text-left">
              <th className="p-2">{t.colName}</th>
              <th className="p-2">{t.colSize}</th>
              <th className="p-2">{t.colTime}</th>
              <th className="p-2">{t.colAction}</th>
            </tr>
          </thead>
          <tbody>
            {uploads.map((u) => (
              <tr key={u.id} className="border-t border-border">
                <td className="p-2 mono break-all">{u.stored_filename}</td>
                <td className="p-2 mono text-fg-2">{fmtBytes(u.size_bytes)}</td>
                <td className="p-2 text-fg-3">{fmtTime(u.created_at)}</td>
                <td className="p-2">
                  <button
                    onClick={() => onDelete(u.id)}
                    disabled={deleting === u.id}
                    className="text-[11px] px-2 py-0.5 rounded border border-red-500/50 text-red-400 hover:bg-red-500/10 disabled:opacity-50"
                  >
                    {deleting === u.id ? t.deleting : t.deleteBtn}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {error && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 p-3 text-[12px] text-red-300" role="alert">
          {error}
        </div>
      )}

      <Modal
        open={!!pendingDelete}
        onClose={() => setPendingDelete(null)}
        title={t.confirmDeleteTitle}
        width={420}
        footer={
          <>
            <button
              onClick={() => setPendingDelete(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
            >
              {t.confirmCancelBtn}
            </button>
            <button
              onClick={confirmDelete}
              className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90"
            >
              {t.confirmDeleteBtn}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">{t.confirmDelete}</p>
      </Modal>
    </div>
  );
}
