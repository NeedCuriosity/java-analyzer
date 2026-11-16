//- /app/Animal.java
package app;

public class Animal {
    protected String name;
}

//- /app/Pet.java
package app;

public class Pet extends Animal {
    protected String owner;

    void feed() {
    }
}

//- /app/Dog.java
package app;

public class Dog extends Pet {
    String /*@describe*/describe() {
        return "dog " + name;
    }

    String /*@ownedBy*/ownedBy() {
        return owner;
    }

    void /*@feed*/feed() {
    }
}

//? assist pull_up_method describe ; has "public class Pet extends Animal {\n    protected String owner;\n\n    void feed() {\n    }\n\n    String describe() {"
//? assist pull_up_method#2 describe ; has "public class Animal {\n    protected String name;\n\n    String describe() {\n        return \"dog \" + name;\n    }\n}" ; lacks "Dog extends Pet {\n    String /*@describe*/describe()"
//? assist pull_up_method ownedBy ; has "    String ownedBy() {\n        return owner;\n    }\n}"
//? noassist pull_up_method#2 ownedBy
//? noassist pull_up_method feed
