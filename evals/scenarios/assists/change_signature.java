//- /app/Shape.java before
package app;

public abstract class Shape {
    /**
     * Scales the shape.
     *
     * @param factor how much
     * @param label a label
     */
    public abstract void scale(double factor, String label);
}
//- /app/Shape.java
package app;

public abstract class Shape {
    /**
     * Scales the shape.
     *
     * @param factor how much
     * @param label a label
     */
    public abstract void /*@decl*/scale(String label, double factor, boolean round);
}
//- /app/Circle.java
package app;

public class Circle extends Shape {
    @Override
    public void scale(double factor, String label) {
        System.out.println(label + factor);
    }
}
//- /app/Main.java
package app;

import java.util.List;

class Main {
    void run(Shape shape, List<Shape> shapes) {
        shape.scale(2.0, "twice");
        Shape anon = new Shape() {
            @Override
            public void scale(double f, String l) {
                super.toString();
            }
        };
        anon.scale(0.5, "half");
        boolean round = true;
        new Circle().scale(3.0, "three");
        /*@body*/shapes.forEach(s -> s.scale(1.0, "one"));
    }
}
//? assist change_signature decl ; has "public abstract void scale(String label, double factor, boolean round);" ; has "     * @param label a label\n     * @param factor how much\n     * @param round\n     */" ; has "public void scale(String label, double factor, boolean round) {" ; has "public void scale(String l, double f, boolean round) {" ; has "shape.scale(\"twice\", 2.0, false);" ; has "anon.scale(\"half\", 0.5, false);" ; has "new Circle().scale(\"three\", 3.0, round);" ; has "s -> s.scale(\"one\", 1.0, round)"
//? noassist change_signature body
