import { cn } from "@/shared/lib/cn";
import {
  DEVICE_ROBOT_COLORS,
  DEVICE_ROBOT_SHAPES,
  type DeviceRobotOverride,
  type DeviceRobotVariant,
  resolveDeviceRobotVariant,
} from "@/shared/lib/deviceRobot";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";

/** Names for `DEVICE_ROBOT_COLORS`, in palette order. */
const COLOR_NAMES = [
  "Red",
  "Orange",
  "Green",
  "Teal",
  "Blue",
  "Indigo",
  "Violet",
  "Pink",
] as const;

function shapeName(shape: string) {
  return shape.charAt(0).toUpperCase() + shape.slice(1);
}

function Choice({
  label,
  onClick,
  selected,
  disabled,
  variant,
}: {
  label: string;
  onClick: () => void;
  selected: boolean;
  disabled: boolean;
  variant: DeviceRobotVariant | null;
}) {
  return (
    <button
      aria-label={label}
      aria-pressed={selected}
      className={cn(
        "flex h-8 w-8 items-center justify-center rounded-md border transition-colors hover:bg-muted focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
        selected ? "border-foreground/60 bg-muted" : "border-transparent",
      )}
      disabled={disabled}
      onClick={onClick}
      title={label}
      type="button"
    >
      <DeviceRobotIcon className="h-5 w-5" variant={variant} />
    </button>
  );
}

/**
 * The device's robot as a button that opens a small picker: the six
 * silhouettes (in the current colour) and the eight colours (in the current
 * silhouette), each drawn as the actual robot. Picking one calls `onPick`
 * with the full choice.
 */
export function DeviceRobotPicker({
  deviceId,
  deviceName,
  disabled,
  onPick,
  variant,
}: {
  deviceId: string;
  deviceName: string;
  disabled: boolean;
  onPick: (override: DeviceRobotOverride) => void;
  variant: DeviceRobotVariant | null;
}) {
  if (!variant) {
    return <DeviceRobotIcon className="h-5 w-5" variant={null} />;
  }
  const pick = (override: DeviceRobotOverride) => {
    if (
      override.shape === variant.shape &&
      override.colorIndex === variant.colorIndex
    ) {
      return;
    }
    onPick(override);
  };
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          aria-label={`Change robot for ${deviceName}`}
          className="flex shrink-0 rounded-md focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed"
          data-testid="device-robot-picker-trigger"
          disabled={disabled}
          type="button"
        >
          <DeviceRobotIcon className="h-5 w-5" variant={variant} />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-auto space-y-2 p-3"
        data-testid="device-robot-picker"
      >
        <p className="text-xs font-medium text-muted-foreground">Shape</p>
        <div className="flex gap-1">
          {DEVICE_ROBOT_SHAPES.map((shape) => (
            <Choice
              disabled={disabled}
              key={shape}
              label={`${shapeName(shape)} robot`}
              onClick={() => pick({ shape, colorIndex: variant.colorIndex })}
              selected={shape === variant.shape}
              variant={resolveDeviceRobotVariant(deviceId, {
                shape,
                colorIndex: variant.colorIndex,
              })}
            />
          ))}
        </div>
        <p className="text-xs font-medium text-muted-foreground">Colour</p>
        <div className="flex gap-1">
          {DEVICE_ROBOT_COLORS.map((_, colorIndex) => (
            <Choice
              disabled={disabled}
              key={COLOR_NAMES[colorIndex]}
              label={`${COLOR_NAMES[colorIndex]} robot`}
              onClick={() => pick({ shape: variant.shape, colorIndex })}
              selected={colorIndex === variant.colorIndex}
              variant={resolveDeviceRobotVariant(deviceId, {
                shape: variant.shape,
                colorIndex,
              })}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
