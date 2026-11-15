package app;

record /*@point*/Point(int /*@x*/x, int /*@y*/y) {
    int /*@sum*/sum() { return /*@use_x*/x + this./*@use_y*/y; }
}

sealed interface Shape permits Circle, Rect {}
record Circle(Point center, int /*@radius*/radius) implements Shape {}
record Rect(Point /*@corner*/corner, int w, int h) implements Shape {}

class Main {
    int run(Object o, Shape shape) {
        if (o instanceof Point /*@p*/p && /*@use_p*/p.x() > 0) {
            return p./*@call_x*/x() + p./*@call_sum*/sum();
        }
        return switch (shape) {
            case Circle /*@c*/c -> /*@use_c*/c./*@call_radius*/radius();
            case Rect(Point /*@corner_var*/corner, int w, int h) -> /*@use_corner*/corner.y() + w + h;
        };
    }

    /*@new_point_ty*/Point make() { return new Point(1, 2); }
}
//? def use_x -> x
//? def use_y -> y
//? def use_p -> p
//? def call_x -> x
//? def call_sum -> sum
//? def use_c -> c
//? def call_radius -> radius
//? def use_corner -> corner_var
//? def new_point_ty -> point
//? diag none
