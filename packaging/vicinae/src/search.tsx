import { Action, ActionPanel, List } from "@vicinae/api";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { useEffect, useState } from "react";

const execFileAsync = promisify(execFile);

type Hit = { name: string; path: string; dir: boolean };

async function qfind(query: string): Promise<Hit[]> {
  const q = query.trim();
  if (!q) return [];
  const { stdout } = await execFileAsync("qfind", ["--json", "--limit", "32", "--files", q], {
    timeout: 2500,
    maxBuffer: 1_000_000,
    env: { ...process.env, RAYON_NUM_THREADS: "1" },
  });
  return stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Hit);
}

export default function Search() {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (!query.trim()) {
      setHits([]);
      setIsLoading(false);
      return;
    }
    setIsLoading(true);
    qfind(query)
      .then((rows) => {
        if (!cancelled) setHits(rows);
      })
      .catch(() => {
        if (!cancelled) setHits([]);
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [query]);

  return (
    <List
      isLoading={isLoading}
      throttle
      onSearchTextChange={setQuery}
      searchBarPlaceholder="Qfind — filename search"
    >
      {hits.length === 0 ? (
        <List.EmptyView
          title={query.trim() ? "No files found" : "Type a name — .wav filters by type"}
          description="Qfind Catalog · .wav .png .exe prefer that extension"
        />
      ) : (
        hits.map((hit) => (
          <List.Item
            key={hit.path}
            title={hit.name}
            subtitle={hit.path}
            icon={{ fileIcon: hit.path }}
            accessories={hit.dir ? [{ text: "folder" }] : []}
            actions={
              <ActionPanel>
                <Action.Open title="Open" target={hit.path} />
                <Action.CopyToClipboard title="Copy Path" content={hit.path} />
                <Action.ShowInFinder title="Show in Files" path={hit.path} />
              </ActionPanel>
            }
          />
        ))
      )}
    </List>
  );
}
