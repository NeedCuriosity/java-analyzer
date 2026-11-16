interface /*@shape*/Shape {
    double /*@area*/area();
}

class /*@sq*/Square implements Shape {
    public double /*@sqArea*/area() { return 4; }
}

class Factory {
    Shape circle() {
        return new /*@anonCls*/Shape() {
            @Override
            public double /*@anonArea*/area() { return 3.14; }
        };
    }

    Shape triangle() {
        class /*@localCls*/Triangle implements Shape {
            public double /*@triArea*/area() { return 0.5; }
        }
        return new Triangle();
    }

    double total(Shape s) {
        return s./*@call*/area();
    }
}
//? impl area -> sqArea anonArea triArea
//? impl call -> sqArea anonArea triArea
//? impl shape -> sq anonCls localCls
//? rename area surface ; has "public double surface() { return 3.14; }" ; has "public double surface() { return 0.5; }" ; has "s.surface()" ; lacks "area"
//? diag none
