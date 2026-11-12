interface Props {
  label: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}

/** A labelled on/off switch. */
export default function Switch({ label, checked, onChange }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={checked ? "sw on" : "sw"}
      onClick={() => onChange(!checked)}
    />
  );
}
