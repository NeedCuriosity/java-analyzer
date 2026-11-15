import java.util.ArrayList;
import java.util.List;

public class Inheritance {
    interface Named {
        String name();

        default String greeting() {
            return "hello " + name();
        }

        static Named of(String value) {
            return () -> value;
        }
    }

    static abstract class Animal implements Named {
        protected final String name;
        protected int legs;

        Animal(String name, int legs) {
            this.name = name;
            this.legs = legs;
        }

        @Override
        public String name() {
            return name;
        }

        abstract String sound();

        String describe() {
            String text = name + " says " + sound();
            return text;
        }
    }

    static class Dog extends Animal {
        Dog(String name) {
            super(name, 4);
        }

        @Override
        String sound() {
            return "woof";
        }

        @Override
        String describe() {
            String base = super.describe();
            return base + " with " + legs + " legs";
        }
    }

    static class Puppy extends Dog {
        Puppy() {
            this("puppy");
        }

        Puppy(String name) {
            super(name);
        }

        @Override
        public String greeting() {
            return super.greeting() + "!";
        }
    }

    public List<String> all() {
        List<Animal> animals = new ArrayList<>();
        animals.add(new Dog("rex"));
        animals.add(new Puppy());
        List<String> out = new ArrayList<>();
        for (Animal animal : animals) {
            String line = animal.describe();
            out.add(line);
        }
        Named named = Named.of("x");
        out.add(named.greeting());
        return out;
    }

    public int legs(Animal animal) {
        Named asNamed = animal;
        Object asObject = asNamed;
        int count = animal.legs;
        if (asObject instanceof Dog dog) {
            count += dog.legs;
        }
        return count;
    }
}
