//- /app/Prim.java
package app;

public class Prim {
    String f(int x) {
        return switch (x) {
            case 0 -> "zero";
            case int i -> "other";
        };
    }

    boolean g(long v) {
        return v instanceof int i && i > 0;
    }

    String widening(byte b) {
        return switch (b) {
            case 1 -> "one";
            case long l -> "long " + l;
        };
    }

    String boxed(Integer value) {
        return switch (value) {
            case int i -> "int " + i;
        };
    }

    String bool(boolean flag) {
        return switch (flag) {
            case true -> "yes";
            case false -> "no";
        };
    }

    String doubles(double d) {
        return switch (d) {
            case 1.0 -> "one";
            case double other -> "other";
        };
    }

    record Point(int x, int y) {}

    double length(Point p) {
        return switch (p) {
            case Point(double x, double y) -> Math.sqrt(x * x + y * y);
        };
    }

    String objects(Object o) {
        if (o instanceof int i) {
            return "int " + i;
        }
        return "other";
    }
}
//? level 25 preview
//? diag none
