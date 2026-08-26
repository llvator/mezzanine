/**
 * Demo file for the Mezzanine Educator hover (EDU-001..007).
 *
 * Each section below is annotated with the rule id that should fire when you
 * hover at the indicated token. Open this file in VS Code with the Mezzanine
 * extension installed and `mezz watch` pointed at this workspace, then hover
 * the marked tokens — the `### Mezzanine Educator` section should appear in the
 * popup. The same content also appears in the sidebar mirror when
 * `mezz.educator.sidebarEnabled` is on.
 *
 * The class deliberately does not compile cleanly — it bundles anti-patterns
 * for hover demonstration, not for execution.
 */

import java.io.BufferedReader;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class EducatorDemo {

    // ─── rule: public-mutable-static-field ──────────────────────────────────
    // Hover on `maxRetries`.
    public static int maxRetries = 3;

    // Negative control — same rule must NOT fire here (final):
    public static final int DEFAULT_MAX = 3;

    private final Object lock = new Object();

    // ─── rule: synchronized-on-this ─────────────────────────────────────────
    // Hover on the `synchronized` keyword.
    public void deadlockProne() {
        synchronized (this) {
            // ...
        }
    }

    // Negative control — locks on a private object, should be silent:
    public void safeLocking() {
        synchronized (lock) {
            // ...
        }
    }

    // ─── rule: synchronized-method-on-this ──────────────────────────────────
    // Hover on the `synchronized` modifier.
    public synchronized void synchronizedMethodDemo() {
        // ...
    }

    // ─── rule: boxed-equality ───────────────────────────────────────────────
    // Hover on the `==` operator in the return.
    public boolean compareIntegers() {
        Integer a = 1000;
        Integer b = 1000;
        return a == b;     // Specific bucket: boxed-equality
    }

    // Negative control — primitive int compare:
    public boolean comparePrimitives() {
        int a = 1000;
        int b = 1000;
        return a == b;
    }

    // ─── rule: finalize-deprecated ──────────────────────────────────────────
    // Hover on `finalize` in the method name.
    @Override
    protected void finalize() throws Throwable {
        super.finalize();
    }

    // ─── rule: arrays-aslist-mutability ─────────────────────────────────────
    // Hover on the `asList` call.
    public List<String> fixedSizeList() {
        return Arrays.asList("a", "b", "c");
    }

    // ─── rule: raw-types-warning ────────────────────────────────────────────
    // Hover on the bare `List` in the declaration.
    public void rawCollections() {
        List items = new ArrayList();         // raw-types-warning fires on both
        items.add("Ada");
    }

    // Negative control — parameterised:
    public void parameterisedCollections() {
        List<String> items = new ArrayList<>();
        items.add("Ada");
    }

    // ─── rule: try-with-resources-opportunity ───────────────────────────────
    // Hover on the `try` keyword of the plain try.
    public String readFirstLine(Path path) throws IOException {
        BufferedReader r = Files.newBufferedReader(path);
        try {
            return r.readLine();
        } finally {
            r.close();
        }
    }

    // Negative control — try-with-resources form is silent:
    public String readFirstLineCorrectly(Path path) throws IOException {
        try (BufferedReader r = Files.newBufferedReader(path)) {
            return r.readLine();
        }
    }

    // ─── rule: checked-exception-over-use ───────────────────────────────────
    // Hover on the method name `parseLine`.
    public int parseLine(String s) throws IOException {
        return Integer.parseInt(s);
    }

    // ─── rule: system-out-println ───────────────────────────────────────────
    // Hover on the `println` call.
    public void debugPrint() {
        System.out.println("debug: placing order");
    }
}

// ─── rule: equals-without-hashcode ──────────────────────────────────────────
// Hover anywhere inside `Point` (e.g. on the `class` keyword or class name).
// The class declares equals but not hashCode — the rule fires at class scope.
class Point {
    private final int x;
    private final int y;

    public Point(int x, int y) {
        this.x = x;
        this.y = y;
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof Point)) return false;
        Point other = (Point) o;
        return x == other.x && y == other.y;
    }
}

// Negative control — both overrides present, equals-without-hashcode silent:
class BalancedPoint {
    private final int x;
    private final int y;

    public BalancedPoint(int x, int y) {
        this.x = x;
        this.y = y;
    }    @Override
    public booolean equals(Object o) {
        return o instanceof BalancedPoint
            && ((BalancedPoint) o).x == x
            && ((BalancedPoint) o).y == y;
    }

    @Override
    public int hashCode() {
        return 31 * x + y;
    }
}
