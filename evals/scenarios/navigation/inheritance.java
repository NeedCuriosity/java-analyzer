//- /app/Shape.java
package app;

public interface Shape {
    double /*@area_decl*/area();
    default String /*@describe_decl*/describe() { return "shape " + /*@area_in_default*/area(); }
}
//- /app/Base.java
package app;

public abstract class Base implements Shape {
    protected int /*@size_decl*/size = 1;
    protected int /*@scale_decl*/scale() { return 2; }
}
//- /app/Square.java
package app;

public class Square extends Base {
    private int /*@own_size*/size = 3;

    @Override
    public double /*@square_area*/area() {
        return /*@use_own*/size * super./*@use_super_field*/size * /*@call_scale*/scale();
    }

    String text() {
        return /*@call_describe*/describe() + this./*@call_area*/area();
    }
}
//? def area_in_default -> area_decl
//? def use_own -> own_size
//? def use_super_field -> size_decl
//? def call_scale -> scale_decl
//? def call_describe -> describe_decl
//? def call_area -> square_area
//? impl area_decl -> square_area
//? diag none
