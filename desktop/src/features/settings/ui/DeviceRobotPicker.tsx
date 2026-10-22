import { type MessageKey, useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import {
  DEVICE_ROBOT_PAINTS,
  DEVICE_ROBOT_SHAPES,
  type DeviceRobotOverride,
  type DeviceRobotShape,
  type DeviceRobotVariant,
  resolveDeviceRobotVariant,
} from "@/shared/lib/deviceRobot";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";

/** Choice labels ("Dome robot"), by shape and by paint name. */
const SHAPE_LABELS: Record<DeviceRobotShape, MessageKey> = {
  classic: "settings.robotPicker.shapeClassic",
  dome: "settings.robotPicker.shapeDome",
  boxy: "settings.robotPicker.shapeBoxy",
  visor: "settings.robotPicker.shapeVisor",
  tall: "settings.robotPicker.shapeTall",
  hex: "settings.robotPicker.shapeHex",
};

const PAINT_LABELS: Record<string, MessageKey> = {
  red: "settings.robotPicker.paintRed",
  green: "settings.robotPicker.paintGreen",
  violet: "settings.robotPicker.paintViolet",
  orange: "settings.robotPicker.paintOrange",
  blue: "settings.robotPicker.paintBlue",
  yellow: "settings.robotPicker.paintYellow",
  rainbow: "settings.robotPicker.paintRainbow",
  gold: "settings.robotPicker.paintGold",
  silver: "settings.robotPicker.paintSilver",
};

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
 * silhouettes (in the current paint) and the palette's colours and gradients
 * (in the current silhouette), each drawn as the actual robot. Picking one calls `onPick`
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
  const t = useT();
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
          aria-label={t("settings.robotPicker.change", { name: deviceName })}
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
        <p className="text-xs font-medium text-muted-foreground">
          {t("settings.robotPicker.shape")}
        </p>
        <div className="flex gap-1">
          {DEVICE_ROBOT_SHAPES.map((shape) => (
            <Choice
              disabled={disabled}
              key={shape}
              label={t(SHAPE_LABELS[shape])}
              onClick={() => pick({ shape, colorIndex: variant.colorIndex })}
              selected={shape === variant.shape}
              variant={resolveDeviceRobotVariant(deviceId, {
                shape,
                colorIndex: variant.colorIndex,
              })}
            />
          ))}
        </div>
        <p className="text-xs font-medium text-muted-foreground">
          {t("settings.robotPicker.colour")}
        </p>
        <div className="flex gap-1">
          {DEVICE_ROBOT_PAINTS.map((paint, colorIndex) => (
            <Choice
              disabled={disabled}
              key={paint.name}
              label={t(PAINT_LABELS[paint.name])}
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
