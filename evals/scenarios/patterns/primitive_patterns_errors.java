//- /app/Prim.java
package app;

public class Prim {
    String notExact(int x) {
        return /*@notexact*/switch (x) {
            case float f -> "float";
        };
    }

    String onlyTrue(boolean flag) {
        return /*@onlytrue*/switch (flag) {
            case true -> "yes";
        };
    }

    String dominated(int x) {
        return switch (x) {
            case int i -> "int";
            case /*@dom*/0 -> "zero";
        };
    }

    String withDefault(int x) {
        return switch (x) {
            case long l -> "long";
            /*@both*/default -> "other";
        };
    }

    String refDominated(Object o) {
        return switch (o) {
            case CharSequence cs -> "chars";
            case /*@domref*/String s -> "string";
            default -> "other";
        };
    }

    boolean inapplicable(String s) {
        return s instanceof /*@inapp*/int i;
    }

    String inapplicableCase(boolean flag) {
        return switch (flag) {
            case /*@boolint*/int i -> "int";
            default -> "x";
        };
    }

    record Point(int x, int y) {}

    String partial(Point p) {
        return /*@partial*/switch (p) {
            case Point(float x, int y) -> "float";
        };
    }
}
//? level 25 preview
//? diag non-exhaustive-switch notexact
//? diag non-exhaustive-switch onlytrue
//? diag dominated-case dom
//? diag dominated-case both
//? diag dominated-case domref
//? diag type-mismatch inapp
//? diag type-mismatch boolint
//? diag non-exhaustive-switch partial
//? diag exact
