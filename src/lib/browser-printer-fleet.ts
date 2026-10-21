import { BrowserPrinterNetwork } from "./browser-printer-network";
import {
  formatWireInteger,
  LibraryHttpError,
  type LibraryClient,
  type BrowserPrinter,
  type BrowserPrinterInput,
  type BrowserMaintenanceLog,
  type BrowserMaintenanceInput,
  type BrowserReminder,
  type BrowserReminderInput,
  type BrowserPrintAttempt,
} from "./library-client";

const DISPLAY_LIMIT = 100;
export function filterFleetHistory<T extends { printerId: string | null }>(
  items: readonly T[],
  selected: string,
): T[] {
  return items.filter(
    (item) =>
      selected === "all" ||
      (selected === "unlinked" ? item.printerId === null : item.printerId === selected),
  );
}
export function fleetRefreshNotice(failures: readonly string[]): string {
  return failures.length
    ? `Could not refresh ${failures.join(", ")}. Other loaded records remain available. Retry refresh before editing stale records.`
    : "";
}
export function printerStatusSummary(
  printer: Pick<BrowserPrinter, "connectionStatus" | "jobStatus" | "lastCheckedAt">,
): string {
  return printer.lastCheckedAt
    ? `Stored status: ${printer.connectionStatus} · ${printer.jobStatus} · checked ${printer.lastCheckedAt}`
    : "No connection check recorded · job status unknown";
}
export function printerHistoryOptions(
  printers: readonly Pick<BrowserPrinter, "id" | "nickname">[],
  attempt: Pick<BrowserPrintAttempt, "printerId" | "printerNicknameSnapshot"> | null,
): Array<{ value: string; label: string }> {
  const options = [
    {
      value: "",
      label:
        !attempt?.printerId && attempt?.printerNicknameSnapshot
          ? `Keep saved printer: ${attempt.printerNicknameSnapshot}`
          : "No printer",
    },
    ...printers.map((printer) => ({ value: printer.id, label: printer.nickname })),
  ];
  if (attempt?.printerId && !printers.some((printer) => printer.id === attempt.printerId))
    options.push({
      value: attempt.printerId,
      label: `${attempt.printerNicknameSnapshot || "Printer"} (saved history; printer removed)`,
    });
  if (!attempt?.printerId && attempt?.printerNicknameSnapshot)
    options.push({ value: "__clear", label: "Clear saved printer" });
  return options;
}
function button(label: string, action: () => void): HTMLButtonElement {
  const control = document.createElement("button");
  control.type = "button";
  control.className = "btn";
  control.textContent = label;
  control.addEventListener("click", action);
  return control;
}
function paragraph(text: string): HTMLParagraphElement {
  const node = document.createElement("p");
  node.textContent = text;
  return node;
}
function problem(error: unknown): string {
  if (
    error instanceof LibraryHttpError &&
    (error.code === "versionConflict" || error.currentVersion)
  )
    return "This record changed elsewhere. Refresh, review the current values, then try again.";
  return error instanceof Error ? error.message : String(error);
}
/** Preserve the saved instant when a minute-resolution field was not edited. */
export function fleetEditedTimestamp(
  original: string,
  initialDisplay: string,
  currentDisplay: string,
): string {
  return currentDisplay === initialDisplay ? original : new Date(currentDisplay).toISOString();
}
export function reminderCanComplete(status: BrowserReminder["status"]): boolean {
  return status === "active";
}
function localDateTime(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}T${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** Fleet management and explicit host-authorized network actions. */
export class BrowserPrinterFleetDialog {
  private dialog: HTMLDialogElement | null = null;
  private editors = new Set<HTMLDialogElement>();
  private network: BrowserPrinterNetwork | null = null;
  private networkAvailable = false;
  setNetworkAvailable(available: boolean): void {
    this.networkAvailable = available;
  }
  constructor(private readonly client: LibraryClient) {}
  dismiss(): void {
    this.network?.dismiss();
    for (const editor of this.editors) editor.close();
    this.dialog?.close();
  }
  async show(): Promise<void> {
    if (this.dialog?.open) {
      this.dialog.focus();
      return;
    }
    const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = document.createElement("dialog");
    this.dialog = dialog;
    dialog.className = "browser-metadata-dialog";
    const title = document.createElement("h2");
    title.textContent = "Printer fleet";
    const notice = paragraph(
      "Manage printer profiles, maintenance, and reminders in Free. Connection values are saved for review on the host. Status below is the last recorded snapshot.",
    );
    const message = paragraph("");
    message.setAttribute("role", "status");
    const selector = document.createElement("select");
    selector.setAttribute("aria-label", "Filter printer fleet history");
    const printerList = document.createElement("div");
    const logsList = document.createElement("div");
    const reminderList = document.createElement("div");
    const controls = document.createElement("div");
    controls.className = "browser-toolbar";
    let printers: BrowserPrinter[] = [];
    let logs: BrowserMaintenanceLog[] = [];
    let reminders: BrowserReminder[] = [];
    let ready = false;
    let busy = false;
    let revision = 0;
    let printerLimit = DISPLAY_LIMIT;
    let logLimit = DISPLAY_LIMIT;
    let reminderLimit = DISPLAY_LIMIT;
    const current = () => dialog.open && dialog.isConnected;
    const network = new BrowserPrinterNetwork(
      this.client,
      async (notice) => {
        if (current()) await refresh(notice);
      },
      () => {
        if (current()) render();
      },
    );
    this.network = network;
    const addPrinter = button("Add printer", () => {
      void this.editPrinter(null, async (input) => {
        await this.client.createPrinter(input);
        await refresh("Printer saved.");
      });
    });
    const selectedPrinter = () => printers.find((printer) => printer.id === selector.value);
    const addLog = button("Log maintenance", () => {
      const printer = selectedPrinter();
      if (printer)
        void this.editMaintenance(null, async (input) => {
          await this.client.createPrinterMaintenance(printer.id, input);
          await refresh("Maintenance saved.");
        });
    });
    const addReminder = button("Add reminder", () => {
      const printer = selectedPrinter();
      if (printer)
        void this.editReminder(null, async (input) => {
          await this.client.createPrinterReminder(printer.id, input);
          await refresh("Reminder saved.");
        });
    });
    const refreshButton = button("Refresh fleet", () => {
      void refresh();
    });
    controls.append(addPrinter, selector, addLog, addReminder, refreshButton);
    const logHeading = document.createElement("h3");
    logHeading.textContent = "Maintenance history";
    const reminderHeading = document.createElement("h3");
    reminderHeading.textContent = "Maintenance reminders";
    dialog.append(
      title,
      notice,
      controls,
      message,
      network.element,
      printerList,
      logHeading,
      logsList,
      reminderHeading,
      reminderList,
      button("Close", () => dialog.close()),
    );
    document.body.append(dialog);
    dialog.showModal();
    addPrinter.focus();
    const run = async (action: () => Promise<unknown>, success: string) => {
      if (busy) return;
      busy = true;
      render();
      try {
        await action();
        if (current()) await refresh(success);
      } catch (error) {
        if (current()) message.textContent = problem(error);
      } finally {
        busy = false;
        if (current()) render();
      }
    };
    const render = () => {
      addPrinter.disabled = busy || !ready;
      refreshButton.disabled = busy;
      selector.disabled = busy;
      addLog.disabled = busy || !selectedPrinter();
      addReminder.disabled = busy || !selectedPrinter();
      printerList.replaceChildren();
      logsList.replaceChildren();
      reminderList.replaceChildren();
      const shownPrinters = printers.filter(
        (printer) => selector.value === "all" || printer.id === selector.value,
      );
      for (const printer of shownPrinters.slice(0, printerLimit)) {
        const row = document.createElement("section");
        const heading = document.createElement("h3");
        heading.textContent = printer.nickname;
        row.append(
          heading,
          paragraph(
            [printer.make, printer.model, printer.printerType].filter(Boolean).join(" · ") ||
              "Printer profile",
          ),
          paragraph(
            `${formatWireInteger(printer.printCount)} prints · ${printerStatusSummary(printer)}`,
          ),
        );
        if (printer.host || printer.webUrl)
          row.append(
            paragraph(
              `${printer.host || printer.webUrl} · ${printer.endpointApproved ? "Host endpoint approved" : "Host endpoint review required"}`,
            ),
          );
        if (printer.notes) row.append(paragraph(printer.notes));
        row.append(
          button("Edit printer", () => {
            void this.editPrinter(printer, async (input) => {
              await this.client.updatePrinter(printer.id, printer.metadataVersion, input);
              await refresh("Printer saved. Connection endpoint requires host review.");
            });
          }),
          button("Delete printer", () => {
            if (
              window.confirm(
                `Delete ${printer.nickname}? Saved print and maintenance history keeps its printer snapshot.`,
              )
            )
              void run(
                () => this.client.deletePrinter(printer.id, printer.metadataVersion),
                "Printer deleted; historical snapshots retained.",
              );
          }),
        );
        row.append(network.printerActions(printer));
        printerList.append(row);
      }
      if (!shownPrinters.length)
        printerList.append(
          paragraph(
            selector.value === "unlinked"
              ? "Unlinked history is shown below."
              : "No printers in this view. Add a printer to start recording maintenance.",
          ),
        );
      if (shownPrinters.length > printerLimit)
        printerList.append(
          button(`Show more printers (${printerLimit} of ${shownPrinters.length})`, () => {
            printerLimit += DISPLAY_LIMIT;
            render();
          }),
        );
      const visibleLogs = filterFleetHistory(logs, selector.value);
      for (const log of visibleLogs.slice(0, logLimit)) {
        const row = document.createElement("section");
        row.append(
          paragraph(
            `${log.performedAt} · ${log.title} · ${log.printerNickname || "Unnamed printer"}${log.printerId ? "" : " (unlinked)"}`,
          ),
        );
        if (log.notes) row.append(paragraph(log.notes));
        row.append(
          button("Edit maintenance", () => {
            void this.editMaintenance(log, async (input) => {
              await this.client.updatePrinterMaintenance(log.id, log.metadataVersion, input);
              await refresh("Maintenance saved.");
            });
          }),
          button("Delete maintenance", () => {
            if (window.confirm(`Delete maintenance record “${log.title}”?`))
              void run(
                () => this.client.deletePrinterMaintenance(log.id, log.metadataVersion),
                "Maintenance deleted.",
              );
          }),
        );
        logsList.append(row);
      }
      if (!visibleLogs.length) logsList.append(paragraph("No maintenance records in this view."));
      if (visibleLogs.length > logLimit)
        logsList.append(
          button(`Show more maintenance (${logLimit} of ${visibleLogs.length})`, () => {
            logLimit += DISPLAY_LIMIT;
            render();
          }),
        );
      const visibleReminders = filterFleetHistory(reminders, selector.value);
      for (const reminder of visibleReminders.slice(0, reminderLimit)) {
        const row = document.createElement("section");
        row.append(
          paragraph(
            `${reminder.title} · ${reminder.status} · due ${reminder.dueAt} · ${reminder.printerNickname || "Unnamed printer"}${reminder.printerId ? "" : " (unlinked)"}${reminder.intervalDays ? ` · every ${reminder.intervalDays} days` : ""}`,
          ),
        );
        if (reminder.notes) row.append(paragraph(reminder.notes));
        row.append(
          button("Edit reminder", () => {
            void this.editReminder(reminder, async (input) => {
              await this.client.updatePrinterReminder(reminder.id, reminder.metadataVersion, input);
              await refresh("Reminder saved.");
            });
          }),
        );
        if (reminderCanComplete(reminder.status))
          row.append(
            button("Complete reminder", () => {
              void this.editCompletion(reminder, async (completedAt, notes) => {
                await this.client.completePrinterReminder(reminder.id, reminder.metadataVersion, {
                  completedAt,
                  notes,
                });
                await refresh("Reminder completed and maintenance logged.");
              });
            }),
          );
        if (reminder.status === "paused")
          row.append(paragraph("Resume this reminder before completing it."));
        if (reminder.status !== "completed")
          row.append(
            button(reminder.status === "paused" ? "Resume reminder" : "Pause reminder", () => {
              void run(
                () =>
                  this.client.pausePrinterReminder(
                    reminder.id,
                    reminder.metadataVersion,
                    reminder.status !== "paused",
                  ),
                "Reminder updated.",
              );
            }),
          );
        row.append(
          button("Delete reminder", () => {
            if (window.confirm(`Delete reminder “${reminder.title}”?`))
              void run(
                () => this.client.deletePrinterReminder(reminder.id, reminder.metadataVersion),
                "Reminder deleted.",
              );
          }),
        );
        reminderList.append(row);
      }
      if (!visibleReminders.length) reminderList.append(paragraph("No reminders in this view."));
      if (visibleReminders.length > reminderLimit)
        reminderList.append(
          button(`Show more reminders (${reminderLimit} of ${visibleReminders.length})`, () => {
            reminderLimit += DISPLAY_LIMIT;
            render();
          }),
        );
      if (busy)
        for (const control of dialog.querySelectorAll<HTMLButtonElement>("section button"))
          control.disabled = true;
    };
    const refresh = async (success = "") => {
      const request = ++revision;
      message.textContent = "Loading fleet…";
      const result = await Promise.allSettled([
        this.client.listPrinters(),
        this.client.listPrinterMaintenance(),
        this.client.listPrinterReminders(),
      ]);
      if (!current() || request !== revision) return;
      const failures: string[] = [];
      if (result[0].status === "fulfilled") {
        printers = result[0].value;
        ready = true;
      } else failures.push(`printers (${problem(result[0].reason)})`);
      if (result[1].status === "fulfilled") logs = result[1].value;
      else failures.push(`maintenance logs (${problem(result[1].reason)})`);
      if (result[2].status === "fulfilled") reminders = result[2].value;
      else failures.push(`reminders (${problem(result[2].reason)})`);
      const selected = selector.value || "all";
      selector.replaceChildren();
      for (const [value, label] of [
        ["all", "All printers"],
        ["unlinked", "Unlinked history"],
        ...printers.map((printer) => [printer.id, printer.nickname]),
      ]) {
        const option = document.createElement("option");
        option.value = value!;
        option.textContent = label!;
        selector.append(option);
      }
      selector.value = [...selector.options].some((option) => option.value === selected)
        ? selected
        : "all";
      message.textContent =
        [success, fleetRefreshNotice(failures)].filter(Boolean).join(" ") ||
        `${printers.length} printer(s). Choose a printer to add maintenance or reminders.`;
      render();
    };
    selector.addEventListener("change", () => {
      printerLimit = DISPLAY_LIMIT;
      logLimit = DISPLAY_LIMIT;
      reminderLimit = DISPLAY_LIMIT;
      render();
    });
    dialog.addEventListener(
      "close",
      () => {
        revision += 1;
        network.dismiss();
        if (this.network === network) this.network = null;
        dialog.remove();
        if (this.dialog === dialog) this.dialog = null;
        if (invoker?.isConnected) invoker.focus();
      },
      { once: true },
    );
    await refresh();
    if (current()) await network.loadPolicy(this.networkAvailable);
  }
  private async editor(
    titleText: string,
    build: (form: HTMLFormElement) => () => Promise<void>,
  ): Promise<void> {
    const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = document.createElement("dialog");
    dialog.className = "browser-metadata-dialog";
    const title = document.createElement("h2");
    title.textContent = titleText;
    const form = document.createElement("form");
    const status = paragraph("");
    status.setAttribute("role", "status");
    const save = button("Save", () => {});
    save.type = "submit";
    const submit = build(form);
    form.append(save);
    const cancel = button("Cancel", () => dialog.close());
    dialog.append(title, form, status, cancel);
    document.body.append(dialog);
    this.editors.add(dialog);
    dialog.showModal();
    form.querySelector<HTMLElement>("input,textarea,select")?.focus();
    let pending = false;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      if (pending) return;
      pending = true;
      save.disabled = true;
      cancel.disabled = true;
      status.textContent = "Saving…";
      void submit()
        .then(() => dialog.close())
        .catch((error) => {
          if (dialog.open) status.textContent = problem(error);
        })
        .finally(() => {
          pending = false;
          save.disabled = false;
          cancel.disabled = false;
        });
    });
    dialog.addEventListener("cancel", (event) => {
      if (pending) event.preventDefault();
    });
    dialog.addEventListener(
      "close",
      () => {
        this.editors.delete(dialog);
        dialog.remove();
        if (invoker?.isConnected) invoker.focus();
        else this.dialog?.querySelector<HTMLElement>("button")?.focus();
      },
      { once: true },
    );
  }
  private field(
    form: HTMLFormElement,
    labelText: string,
    value: string,
    multiline = false,
    type = "text",
  ): HTMLInputElement | HTMLTextAreaElement {
    const label = document.createElement("label");
    label.textContent = labelText;
    const input = multiline ? document.createElement("textarea") : document.createElement("input");
    if (input instanceof HTMLInputElement) input.type = type;
    input.value = value;
    label.append(input);
    form.append(label);
    return input;
  }
  private async editPrinter(
    printer: BrowserPrinter | null,
    save: (input: BrowserPrinterInput) => Promise<void>,
  ): Promise<void> {
    await this.editor(printer ? "Edit printer" : "Add printer", (form) => {
      const name = this.field(form, "Printer nickname", printer?.nickname ?? "");
      name.required = true;
      name.maxLength = 200;
      const make = this.field(form, "Make", printer?.make ?? "");
      const model = this.field(form, "Model", printer?.model ?? "");
      const kind = this.field(form, "Printer type", printer?.printerType ?? "");
      const firmware = this.field(form, "Firmware", printer?.firmware ?? "");
      const host = this.field(form, "Host address", printer?.host ?? "");
      const web = this.field(form, "Web URL", printer?.webUrl ?? "");
      const bed = [0, 1, 2].map((index) => {
        const input = this.field(
          form,
          `Bed ${["width", "depth", "height"][index]} (mm)`,
          printer?.bedMm?.[index]?.toString() ?? "",
          false,
          "number",
        ) as HTMLInputElement;
        input.min = "0.001";
        input.max = "100000";
        input.step = "any";
        return input;
      });
      const notes = this.field(form, "Notes", printer?.notes ?? "", true);
      form.append(
        paragraph(
          "Saving a new or edited profile requires endpoint review on the host before a connection check.",
        ),
      );
      return async () => {
        const values = bed.map((field) => field.value.trim());
        const bedMm = values.every((value) => !value) ? null : values.map(Number);
        if (
          bedMm &&
          (values.some((value) => !value) ||
            bedMm.some((value) => !Number.isFinite(value) || value <= 0 || value > 100000))
        )
          throw new Error("Enter all three positive bed dimensions, or leave all three blank.");
        const optional = (value: string) => value.trim() || null;
        await save({
          nickname: name.value.trim(),
          make: optional(make.value),
          model: optional(model.value),
          printerType: optional(kind.value),
          firmware: optional(firmware.value),
          host: optional(host.value),
          webUrl: optional(web.value),
          bedMm: bedMm as [number, number, number] | null,
          notes: notes.value,
        });
      };
    });
  }
  private async editMaintenance(
    log: BrowserMaintenanceLog | null,
    save: (input: BrowserMaintenanceInput) => Promise<void>,
  ): Promise<void> {
    await this.editor(log ? "Edit maintenance" : "Log maintenance", (form) => {
      const title = this.field(form, "Task", log?.title ?? "");
      title.required = true;
      const originalTimestamp = log?.performedAt ?? new Date().toISOString();
      const initialDisplay = localDateTime(originalTimestamp);
      const date = this.field(form, "Performed at", initialDisplay, false, "datetime-local");
      date.required = true;
      const notes = this.field(form, "Notes", log?.notes ?? "", true);
      return async () => {
        await save({
          title: title.value,
          performedAt: fleetEditedTimestamp(originalTimestamp, initialDisplay, date.value),
          notes: notes.value,
        });
      };
    });
  }
  private async editReminder(
    reminder: BrowserReminder | null,
    save: (input: BrowserReminderInput) => Promise<void>,
  ): Promise<void> {
    await this.editor(reminder ? "Edit reminder" : "Add reminder", (form) => {
      const title = this.field(form, "Task", reminder?.title ?? "");
      title.required = true;
      const originalTimestamp = reminder?.dueAt ?? new Date().toISOString();
      const initialDisplay = localDateTime(originalTimestamp);
      const date = this.field(form, "Due at", initialDisplay, false, "datetime-local");
      date.required = true;
      const interval = this.field(
        form,
        "Repeat every days (blank for once)",
        reminder?.intervalDays?.toString() ?? "",
        false,
        "number",
      ) as HTMLInputElement;
      interval.min = "1";
      interval.max = "36500";
      interval.step = "1";
      const notes = this.field(form, "Notes", reminder?.notes ?? "", true);
      return async () => {
        const intervalDays = interval.value.trim() ? Number(interval.value) : null;
        if (
          intervalDays !== null &&
          (!Number.isInteger(intervalDays) || intervalDays < 1 || intervalDays > 36500)
        )
          throw new Error("Repeat interval must be a whole number from 1 to 36,500 days.");
        await save({
          title: title.value,
          dueAt: fleetEditedTimestamp(originalTimestamp, initialDisplay, date.value),
          intervalDays,
          notes: notes.value,
        });
      };
    });
  }
  private async editCompletion(
    reminder: BrowserReminder,
    save: (completedAt: string, notes: string) => Promise<void>,
  ): Promise<void> {
    await this.editor(`Complete ${reminder.title}`, (form) => {
      const date = this.field(
        form,
        "Completed at",
        localDateTime(new Date().toISOString()),
        false,
        "datetime-local",
      );
      date.required = true;
      const notes = this.field(form, "Completion notes", "", true);
      form.append(paragraph("Completion records maintenance and advances repeating reminders."));
      return async () => {
        await save(new Date(date.value).toISOString(), notes.value);
      };
    });
  }
}
