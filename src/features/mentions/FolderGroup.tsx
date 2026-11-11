import { displayPath, folderName } from "./folder";
import "./mentions.css";

interface Props {
  folder: string | null;
  onOpen: () => void;
  onClose: () => void;
}

/** The sidebar's Folder group: the chat's folder with Change and Close, or a button to open one. */
export default function FolderGroup({ folder, onOpen, onClose }: Props) {
  return (
    <section className="folder-grp" aria-label="Folder">
      <div className="grp">Folder</div>
      {folder ? (
        <>
          <div className="folder-name" title={displayPath(folder)}>
            {folderName(folder)}
          </div>
          <div className="folder-acts">
            <button type="button" onClick={onOpen}>
              Change
            </button>
            <button type="button" onClick={onClose}>
              Close folder
            </button>
          </div>
        </>
      ) : (
        <button type="button" className="folder-open" onClick={onOpen}>
          Open a folder
        </button>
      )}
    </section>
  );
}
