//- /app/Base.java
package app;

public abstract class Base {
    private int secret;
    public final int limit = 3;
    protected int shared;

    private void hidden() {}
    public abstract String name();
    public abstract int size();
    public final void locked() {}
    public Object value() { return null; }
    protected void hook() {}
    public int count() { return 0; }
}
//- /app/Shape.java
package app;

interface Shape {
    double area();
    default String label() { return "shape"; }
}
//- /app/Main.java
package app;

class /*@missing_impl*/Partial extends Base {
    public String name() { return "p"; }
}

class /*@missing_iface*/NoArea implements Shape {
}

class Complete extends Base implements Shape, Comparable<Complete> {
    public String name() { return "c"; }
    public int size() { return 1; }
    public double area() { return 2; }
    public int compareTo(Complete other) { return 0; }

    public void /*@final_override*/locked() {}
    void /*@weaker*/hook() {}
    public String /*@bad_ret*/count() { return ""; }
    public String value() { return "covariant is fine"; }
}

abstract class StillAbstract extends Base {
}

record Point(int x, int y) implements Shape {
    public double area() { return x * y; }
}

class Main {
    int instance;
    static int shared;
    final int fixed = 1;

    int helper() { return 1; }
    static int util() { return 2; }

    static int staticMethod(Main m) {
        int a = /*@static_field*/instance;
        int b = /*@static_call*/helper();
        int c = shared + util() + m.instance + m.helper();
        Runnable r = new Runnable() {
            int own;
            public void run() { own++; }
        };
        r.run();
        return a + b + c;
    }

    void finals(final int param, Base base) {
        final int local = 1;
        /*@final_local*/local = 2;
        /*@final_param*/param++;
        /*@final_field*/fixed = 3;
        base./*@final_lib*/limit = 4;
        final int later;
        later = 5;
        int normal = later;
        normal++;
        instance = normal;
    }

    Main() {
        instance = fixed;
    }

    void access(Base base) {
        int a = base./*@private_field*/secret;
        base./*@private_method*/hidden();
        int b = base.shared + base.limit;
        instance = a + b;
    }

    void returns() {
        /*@value_in_void*/return 1;
    }

    int noValue(boolean flag) {
        if (flag) {
            /*@no_value*/return;
        }
        Runnable r = () -> { return; };
        r.run();
        return 0;
    }

    enum Color { RED, GREEN, BLUE }

    int exhaustive(Color color) {
        int a = /*@not_exhaustive*/switch (color) {
            case RED -> 1;
            case GREEN -> 2;
        };
        int b = switch (color) {
            case RED -> 1;
            case GREEN, BLUE -> 2;
        };
        int c = switch (color) {
            case RED -> 1;
            default -> 2;
        };
        return a + b + c;
    }

    void duplicates(int param, java.util.List<String> names) {
        int x = 1;
        int /*@dup_local*/x = 2;
        String /*@dup_param*/param = "";
        for (String /*@dup_foreach*/x : names) {}
        names.forEach(/*@dup_lambda*/x -> {});
        {
            int inner = 1;
        }
        int inner = 2;
        for (int i = 0; i < 1; i++) {}
        for (int i = 0; i < 1; i++) {}
        Runnable r = new Runnable() {
            public void run() { int x = 3; int param = 4; }
        };
        r.run();
    }
}
//? diag missing-implementation missing_impl
//? diag missing-implementation missing_iface
//? diag invalid-override final_override
//? diag invalid-override weaker
//? diag invalid-override bad_ret
//? diag static-context static_field
//? diag static-context static_call
//? diag assign-to-final final_local
//? diag assign-to-final final_param
//? diag assign-to-final final_field
//? diag assign-to-final final_lib
//? diag inaccessible-member private_field
//? diag inaccessible-member private_method
//? diag type-mismatch value_in_void
//? diag type-mismatch no_value
//? diag non-exhaustive-switch not_exhaustive
//? diag duplicate-variable dup_local
//? diag duplicate-variable dup_param
//? diag duplicate-variable dup_foreach
//? diag duplicate-variable dup_lambda
//? diag exact
