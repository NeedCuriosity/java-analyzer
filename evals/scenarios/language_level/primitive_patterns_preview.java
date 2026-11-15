//- /app/Prims.java
package app;

class Prims {
    String describe(Object o, int n) {
        if (o instanceof int i) {
            return "int " + i;
        }
        return switch (n) {
            case byte b -> "byte";
            default -> "other";
        };
    }
}
//? level 25 preview
//? diag only feature-not-available
