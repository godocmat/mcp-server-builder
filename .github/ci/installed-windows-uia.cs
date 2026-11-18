using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Windows.Automation;

internal sealed class InstalledWindowsSmoke
{
    private const string Marker = "m10-installed-webview-synthetic-marker";
    private readonly Process app;
    private AutomationElement document;

    private InstalledWindowsSmoke(Process app)
    {
        this.app = app;
    }

    [MTAThread]
    private static int Main(string[] args)
    {
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true" ||
            Environment.GetEnvironmentVariable("RUNNER_ENVIRONMENT") != "github-hosted")
        {
            Console.Error.WriteLine("Run the installed GUI smoke on a disposable GitHub-hosted Windows runner.");
            return 2;
        }

        try
        {
            using (Process app = Process.GetProcessById(int.Parse(args[0])))
            {
                new InstalledWindowsSmoke(app).Run();
            }
            return 0;
        }
        catch (Exception failure)
        {
            Console.Error.WriteLine("{0}: {1}", failure.GetType().Name, failure.Message);
            return 1;
        }
    }

    private void Run()
    {
        AutomationElement window = WaitFor("the installed application window", () =>
        {
            app.Refresh();
            IntPtr handle = app.MainWindowHandle;
            return handle == IntPtr.Zero ? null : AutomationElement.FromHandle(handle);
        });
        Console.WriteLine("Installed window opened as {0}.", window.Current.ClassName);
        document = WaitFor("the WebView2 document", () =>
            window.FindFirst(TreeScope.Descendants, WithType(ControlType.Document)));

        Invoke(WaitFor("the registered reference catalog", () =>
            FindAction(document, WithType(ControlType.Button),
                name => name == "Catalog sample" || name.StartsWith("Catalog sample ", StringComparison.Ordinal))));
        Click("Playground");

        PrepareReview();
        Click("Cancel launch review");
        ExpectText("the cancelled review", "Launch review cancelled.");

        PrepareReview();
        Click("Approve and start");
        ExpectText("a connection to the bundled worker", "Local server connected.");
        Click("Run saved fixture");
        string result = ExpectText("the saved fixture result",
            "Assertion 1 passed", "Sample item", "[REDACTED]");
        AssertMarkerAbsent(result);

        OpenDisclosure(WaitFor("the protocol trace", () =>
            FindAction(document, Disclosure, name => name == "Protocol trace")));
        Click("Refresh trace");
        AutomationElement response = WaitFor("a tools/call response in the trace", () =>
            FindAction(document, Disclosure, name =>
                Contains(name, "received") && Contains(name, "response") && Contains(name, "tools/call")));
        AutomationElement detail = TreeWalker.RawViewWalker.GetParent(response);
        OpenDisclosure(response);
        string trace = WaitFor("the redacted tool response in the protocol trace", () =>
            ReadWithText(detail, "Correlation ID", "Sample item", "[REDACTED]"));
        AssertMarkerAbsent(trace);
        AssertMarkerAbsent(ReadText(document));

        Click("Stop server");
        ExpectText("the stopped bundled worker", "Local server stopped.");
        Console.WriteLine("Installed WebView2 opened the catalog, cancelled and approved a launch review, replayed a fixture with a redacted trace, and stopped its bundled worker.");
    }

    private void PrepareReview()
    {
        EnableCheckbox("catalog_read");
        EnableCheckbox("Include redacted message contents in the protocol trace");
        Click("Review launch");
        string worker = Path.Combine(Path.GetDirectoryName(app.MainModule.FileName), "mcp-builder.exe");
        ExpectText("the reviewed installed worker, scenario and grant",
            "Review before starting", "known_item", "catalog_read", worker, "--managed-preview-worker");
    }

    private void EnableCheckbox(string label)
    {
        AutomationElement checkbox = WaitFor(label, () => FindAction(document, WithType(ControlType.CheckBox),
            name => name == label || name.StartsWith(label + " ", StringComparison.Ordinal)));
        TogglePattern pattern = (TogglePattern)checkbox.GetCurrentPattern(TogglePattern.Pattern);
        if (pattern.Current.ToggleState != ToggleState.On)
        {
            pattern.Toggle();
            WaitFor(label + " enabled", () =>
                pattern.Current.ToggleState == ToggleState.On ? checkbox : null);
        }
    }

    private void Click(string name)
    {
        Invoke(WaitFor(name, () =>
            FindAction(document, WithType(ControlType.Button), actual => actual == name)));
    }

    private static void Invoke(AutomationElement control)
    {
        ((InvokePattern)control.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
    }

    private static void OpenDisclosure(AutomationElement control)
    {
        object value;
        if (control.TryGetCurrentPattern(ExpandCollapsePattern.Pattern, out value))
        {
            var toggle = (ExpandCollapsePattern)value;
            switch (toggle.Current.ExpandCollapseState)
            {
                case ExpandCollapseState.Expanded:
                case ExpandCollapseState.PartiallyExpanded:
                    return;
                case ExpandCollapseState.Collapsed:
                    toggle.Expand();
                    return;
            }
        }
        Invoke(control);
    }

    private static readonly Condition Disclosure = new OrCondition(
        WithType(ControlType.Button),
        new PropertyCondition(AutomationElement.IsExpandCollapsePatternAvailableProperty, true));

    private static Condition WithType(ControlType type)
    {
        return new PropertyCondition(AutomationElement.ControlTypeProperty, type);
    }

    private static AutomationElement FindAction(AutomationElement root, Condition condition, Predicate<string> matches)
    {
        var enabled = new PropertyCondition(AutomationElement.IsEnabledProperty, true);
        foreach (AutomationElement item in root.FindAll(TreeScope.Descendants, new AndCondition(condition, enabled)))
        {
            string name = Regex.Replace(item.Current.Name, @"\s+", " ").Trim();
            if (matches(name))
                return item;
        }
        return null;
    }

    private string ExpectText(string step, params string[] expected)
    {
        return WaitFor(step, () => ReadWithText(document, expected));
    }

    private static string ReadWithText(AutomationElement root, params string[] expected)
    {
        string text = ReadText(root);
        foreach (string fragment in expected)
        {
            if (!Contains(text, fragment))
                return null;
        }
        return text;
    }

    private static string ReadText(AutomationElement root)
    {
        var result = new StringBuilder();
        object value;
        if (root.TryGetCurrentPattern(TextPattern.Pattern, out value))
            result.AppendLine(((TextPattern)value).DocumentRange.GetText(-1));
        foreach (AutomationElement item in root.FindAll(TreeScope.Descendants, WithType(ControlType.Text)))
            result.AppendLine(item.Current.Name);

        return Regex.Replace(result.ToString(), @"\s+", " ");
    }

    private static bool Contains(string text, string expected)
    {
        return text.IndexOf(expected, StringComparison.OrdinalIgnoreCase) >= 0;
    }

    private static void AssertMarkerAbsent(string text)
    {
        if (Contains(text, Marker))
            throw new InvalidOperationException("The synthetic private fixture value reached the GUI.");
    }

    private T WaitFor<T>(string step, Func<T> query) where T : class
    {
        Console.WriteLine("Waiting for {0}.", step);
        var timer = Stopwatch.StartNew();
        do
        {
            if (app.HasExited)
                throw new InvalidOperationException("The installed app exited with code " + app.ExitCode + ".");
            try
            {
                T value = query();
                if (value != null)
                    return value;
            }
            catch (ElementNotAvailableException)
            {
                // The renderer can replace a control while updating its accessibility tree.
            }
            Thread.Sleep(250);
        } while (timer.Elapsed < TimeSpan.FromSeconds(30));
        throw new TimeoutException("Timed out waiting for " + step + ".");
    }
}
