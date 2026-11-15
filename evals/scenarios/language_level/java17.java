//- /app/Shapes.java
package app;

sealed interface Shape permits Circle, Square {}

record Circle(double r) implements Shape {}

record Square(double side) implements Shape {}

class Uses {
    String describe(Object o) {
        if (o instanceof Circle c) {
            return "circle " + c.r();
        }
        var text = """
            plain
            """;
        return switch (o) {
            case /*@null*/null -> "null";
            case /*@typepat*/Square s /*@guard*/when s.side() > 1 -> "big square";
            case /*@recpat*/Circle(double r) -> "circle";
            default -> text;
        };
    }

    void unnamed(java.util.List<String> names) {
        for (String /*@unnamed*/_ : names) {
            System.out.println();
        }
    }

    /*@prologue*/Uses(int x) {
        /*@stmt*/int y = x + 1;
        this();
    }

    Uses() {}
}
//? level 17
//? diag feature-not-available null
//? diag feature-not-available typepat
//? diag feature-not-available guard
//? diag feature-not-available recpat
//? diag feature-not-available unnamed
//? diag feature-not-available stmt
//? diag only feature-not-available
