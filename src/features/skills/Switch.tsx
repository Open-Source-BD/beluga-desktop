interface Props {
  label: string;
  /** Names the switch by visible text elsewhere (its own aria-label is then left off). */
  labelledBy?: string;
  describedBy?: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}

/** A labelled on/off switch. */
export default function Switch({ label, labelledBy, describedBy, checked, onChange }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={labelledBy ? undefined : label}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className={checked ? "sw on" : "sw"}
      onClick={() => onChange(!checked)}
    />
  );
}
