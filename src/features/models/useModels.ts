import { useCallback, useEffect, useState } from "react";
import { listModels, type ModelInfo } from "../../lib/ipc";
import { chooseModel } from "./defaultModel";

export function readLastModel(): string | null {
  try {
    return localStorage.getItem("lastModel");
  } catch {
    return null;
  }
}

/** The model list, the model in use, and whether the picker is open (it opens when no model can be chosen). */
export function useModels() {
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [modelsFailed, setModelsFailed] = useState(false);
  const [modelId, setModelId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);

  const loadModels = useCallback((force: boolean) => {
    setModels(null);
    setModelsFailed(false);
    listModels(force).then(
      (list) => {
        setModels(list);
        const id = chooseModel(list, [readLastModel()]);
        setModelId((cur) => (cur && list.some((m) => m.id === cur) ? cur : id));
        if (id === null) setPickerOpen(true);
      },
      () => {
        setModels([]);
        setModelsFailed(true);
        setPickerOpen(true);
      },
    );
  }, []);
  useEffect(() => loadModels(false), [loadModels]);

  const chooseFromPicker = (id: string) => {
    setModelId(id);
    try {
      localStorage.setItem("lastModel", id);
    } catch {
      // Remembering the choice is a convenience; ignore storage failures.
    }
  };

  return { models, modelsFailed, modelId, setModelId, pickerOpen, setPickerOpen, loadModels, chooseFromPicker };
}
