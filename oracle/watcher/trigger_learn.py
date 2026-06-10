import argparse
import sys
from pathlib import Path

from oracle.config import WATCH_DIRS, WATCH_EXTENSIONS
from oracle.ingestion.learn import learn_files


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run Oracle LEARN mode for changed files.")
    parser.add_argument("paths", nargs="*")
    parser.add_argument("--root", default=".")
    parser.add_argument("--no-sentence-transformer", action="store_true")
    parser.add_argument("--no-ollama", action="store_true")
    args = parser.parse_args(argv)
    root = Path(args.root)
    paths = [Path(path) for path in args.paths] or collect_watch_files(root)

    count = learn_files(
        paths,
        project_root=root,
        use_sentence_transformer=not args.no_sentence_transformer,
        use_ollama_classifier=not args.no_ollama,
    )
    print(f"Oracle learned {count} file(s).")
    return 0


def collect_watch_files(root: Path) -> list[Path]:
    files = []
    for watch_dir in WATCH_DIRS:
        target = root / watch_dir
        if not target.exists():
            continue
        files.extend(
            path
            for path in target.rglob("*")
            if path.is_file() and path.suffix.lower() in WATCH_EXTENSIONS
        )
    return sorted(files)


if __name__ == "__main__":
    sys.exit(main())
