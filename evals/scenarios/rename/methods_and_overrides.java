//- /app/Shape.java
package app;

interface Shape {
    double /*@iface*/area();
}
//- /app/Square.java
package app;

class Square implements Shape {
    public double /*@impl*/area() { return 4; }
    double area(int scale) { return area() * scale; }
}
//- /app/Main.java
package app;

class Main {
    double run(Shape s, Square q) {
        return s.area() + q.area() + q.area(2);
    }
}
//? rename iface surface ; has "double surface();" ; has "public double surface() { return 4; }" ; has "return surface() * scale;" ; has "s.surface() + q.surface() + q.area(2)" ; has "double area(int scale)"
//? rename impl surface ; has "double surface();" ; has "s.surface() + q.surface() + q.area(2)"
