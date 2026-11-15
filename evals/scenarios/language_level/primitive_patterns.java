//- /app/Prims.java
package app;

class Prims {
    String describe(Object o, int n) {
        if (o instanceof /*@inst*/int i) {
            return "int " + i;
        }
        return switch (n) {
            case /*@case*/byte b -> "byte";
            default -> "other";
        };
    }
}
//? diag feature-not-available inst
//? diag feature-not-available case
//? diag only feature-not-available
