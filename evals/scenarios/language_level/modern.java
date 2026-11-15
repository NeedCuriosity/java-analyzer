//- /app/Modern.java
package app;

class Modern {
    String describe(Object o) {
        return switch (o) {
            case null -> "null";
            case Integer i when i > 1 -> "big";
            case String s -> s;
            default -> "other";
        };
    }

    void unnamed(java.util.List<String> names) {
        for (String _ : names) {
            System.out.println();
        }
    }

    Modern(int x) {
        int y = x + 1;
        this();
    }

    Modern() {}
}
//? diag none
