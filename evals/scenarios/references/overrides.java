//- /app/Shape.java
package app;

public interface Shape {
    double /*@area*/area();
    double perimeter();
}
//- /app/Circle.java
package app;

public class Circle implements Shape {
    public double /*@circle_area*/area() { return 3; }
    public double perimeter() { return 6; }
    // 同名但参数不同：不是覆写
    public double /*@scaled*/area(double scale) { return /*@self_call*/area() * scale; }
}
//- /app/Main.java
package app;

class Main {
    double run(Shape s, Circle c) {
        return s./*@via_iface*/area() + c./*@via_class*/area() + c./*@overload_call*/area(2.0) + s.perimeter();
    }
}
//? refs area = self_call via_iface via_class
//? refs circle_area = self_call via_iface via_class
//? refs scaled = overload_call
//? diag none
